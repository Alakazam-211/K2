//! The packs: each reader as the kit's [`BlobPack`] at tier 0, indexing one version of a file
//! into the kit's structure index ([`IndexDraft`]). Agents never write these files
//! ([`AgentWrites::Refused`]): they are outputs a board design regenerates.
//!
//! **What a node is.** The roots are families, each a fixed node (`@family:<name>`); under them
//! the units:
//!
//! | Family | Unit | Key | Children |
//! |---|---|---|---|
//! | `apertures` | an aperture (`D10`) | `@ap:<definition>` | none |
//! | `macros` | an aperture macro | `@macro:<name>` | none |
//! | `objects` | the objects made with one aperture; the regions | `@objects:<definition>`, `@objects:regions` | none |
//! | `tools` (drill) | a tool (`T01`) | `@tool:<definition>` | its hits (`@hits:<definition>`), its rout paths (`@routs:<definition>`) |
//! | `attributes` | an attribute name, then each value | `@attr:<o or a>.<name>`, `@attrval:<o or a>.<name>=<value>` | values |
//! | `step-repeats` | a step and repeat | `@sr:<ordinal>` | none |
//! | `file-attributes` | a file attribute | `@tf:<name>` | none |
//!
//! A family appears when it has units (`apertures` and `objects` always for a photoplot,
//! `tools` for a drill file). Objects group by the attributes they carry: their own (`o`) and
//! their aperture's or tool's (`a`; a region's are its own aperture attributes).
//!
//! **Keys** come from what the file defines, never from position or a number an exporter
//! reissues. An aperture's key is its definition ([`aperture_ids`]): its template (a macro by
//! name), its parameters and its `.AperFunction` (`@ap:circle,0.25;Conductor`), and a block
//! aperture's is a digest of what it holds (`@ap:block,<16 hex>`); a tool's is its diameter and
//! its `.AperFunction` (`@tool:0.3;ViaDrill`, [`tool_ids`]). Two identical definitions take
//! `~2`, `~3` in number order. The number is the node's `number` attribute (its objects' group:
//! `aperture`) and its name (`D10`, `T01`). So the same bytes give the same keys, and so does a
//! fresh export that renumbers the apertures and tools: a key never names another definition.
//! Macros, attributes and file attributes are keyed by their names and values. Within a family
//! the units are in key order. A name byte outside printable ASCII, a `/` or a `%` is written
//! `%XX`; a key over the kit's length limit is left out, and the kit mints one.
//!
//! **Digests** hold what a node means: an object's shape, polarity, aperture by key,
//! transform and attributes by value; a hit's or rout's points, tool by key and attributes;
//! never a byte offset, an aperture or tool number, or a list position.
//!
//! **What a node holds.** Counts in `attrs`; `bounds` is the box of the points the file states
//! for the node's objects ([`crate::geom`]), in the file's integer units, `left`/`right` the
//! least and greatest x and `top`/`bottom` the least and greatest y (the file's y axis points
//! up: the doc's `y_axis` attribute says so). Attribute values and file attributes are a
//! node's text. The record digest covers the node's own definition; the content digest, a
//! group's objects.

use std::collections::BTreeMap;
use std::io::Read;

use k2_mpc_base::index::{
    cut_text, Digest, DigestStream, DraftNode, IndexDraft, IndexLimits, IndexRefused, IndexSpec,
    IndexVerbs, NodeRect, INDEX_ATTR_VALUE_MAX, INDEX_KEY_MAX, INDEX_NAME_MAX,
};
use k2_mpc_base::session::{AgentWrites, BlobPack};

use crate::attr::Attr;
use crate::drill::{self, Drill, RoutSegment};
use crate::error::{ReadError, WarnCode};
use crate::geom::{Bounds, Pt};
use crate::mutant;
use crate::photoplot::{self, Photoplot, Segment, Shape, Template};
use crate::Limits;

/// The index's output format: bump when what a file indexes to changes, and the kit recomputes.
/// 2: apertures and tools keyed by their definitions.
pub const FORMAT: u32 = 2;

/// The photoplot pack's index verbs.
pub const PHOTOPLOT_VERBS: IndexVerbs = IndexVerbs::new(
    "feature list",
    "feature show",
    "feature diff",
    "feature blame",
    None,
);

/// The drill pack's index verbs.
pub const DRILL_VERBS: IndexVerbs =
    IndexVerbs::new("hole list", "hole show", "hole diff", "hole blame", None);

/// Reads photoplot files.
#[derive(Debug, Clone, Copy, Default)]
pub struct PhotoplotPack;

/// Reads drill files in the XNC subset.
#[derive(Debug, Clone, Copy, Default)]
pub struct DrillPack;

/// Reads drill files strictly as XNC, else as a common export ([`drill::common`]): a file the
/// common reading reads is indexed as not conforming, with the constructs it relied on and the
/// strict reader's refusal in the doc attributes.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommonDrillPack;

/// Reads the whole stream, bounded by the reader's size limit.
fn read_all(src: &mut dyn Read, limits: &Limits) -> Result<Vec<u8>, IndexRefused> {
    let mut bytes = Vec::new();
    src.take(limits.max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(IndexRefused::Read)?;
    if bytes.len() as u64 > limits.max_bytes {
        return Err(IndexRefused::Unreadable {
            why: "limit:bytes (reader limit)".into(),
        });
    }
    Ok(bytes)
}

/// A reader's refusal as the kit stores it: its code and section, never file content.
pub fn refusal(e: &ReadError) -> IndexRefused {
    IndexRefused::Unreadable { why: e.summary() }
}

impl BlobPack for PhotoplotPack {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new("photoplot", FORMAT, &PHOTOPLOT_VERBS))
    }

    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let rl = Limits::default();
        let bytes = read_all(src, &rl)?;
        let m = photoplot::read(&bytes, &rl).map_err(|e| refusal(&e))?;
        index_photoplot(&m, limits)
    }

    /// Agents never write a photoplot file: they propose a new file and a comment.
    fn agent_writes(&self) -> AgentWrites {
        AgentWrites::Refused
    }
}

impl BlobPack for DrillPack {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new("drill", FORMAT, &DRILL_VERBS))
    }

    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let rl = Limits::default();
        let bytes = read_all(src, &rl)?;
        let m = drill::read(&bytes, &rl).map_err(|e| refusal(&e))?;
        index_drill(&m, limits)
    }

    /// Agents never write a drill file: they propose a new file and a comment.
    fn agent_writes(&self) -> AgentWrites {
        AgentWrites::Refused
    }
}

impl BlobPack for CommonDrillPack {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new("drill-common", FORMAT, &DRILL_VERBS))
    }

    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let rl = Limits::default();
        let bytes = read_all(src, &rl)?;
        let strict = match drill::read(&bytes, &rl) {
            Ok(m) => return index_drill(&m, limits),
            Err(e) => e,
        };
        match drill::common::read(&bytes, &rl) {
            Ok(c) => index_common(&c, Some(&strict), limits),
            Err(e) => {
                let why = format!("{}; strict: {}", e.summary(), strict.summary());
                Err(IndexRefused::Unreadable { why })
            }
        }
    }

    /// Agents never write a drill file: they propose a new file and a comment.
    fn agent_writes(&self) -> AgentWrites {
        AgentWrites::Refused
    }
}

/// Indexes a drill file the common reading read: the drill index, with the verdict (not
/// conforming), the number format the header fixed, each construct relied on
/// (`relies.<construct>`, a count) and the strict reader's refusal.
pub fn index_common(
    c: &drill::common::CommonDrill,
    strict: Option<&ReadError>,
    limits: &IndexLimits,
) -> Result<IndexDraft, IndexRefused> {
    let mut d = index_drill(&c.drill, limits)?;
    d.source = "drill-common-export".into();
    let a = &mut d.attrs;
    a.insert("spec".into(), "common-export".into());
    a.insert("conformance".into(), c.conformance().name().into());
    if let Some(f) = c.format {
        let z = match f.zeros {
            drill::common::Zeros::Leading => "leading-kept",
            drill::common::Zeros::Trailing => "trailing-kept",
            drill::common::Zeros::Exact => "exact",
        };
        a.insert(
            "number_format".into(),
            format!("{z}:{}.{}", f.int_digits, f.decimals),
        );
    }
    for (k, n) in &c.relied {
        a.insert(format!("relies.{}", k.name()), n.to_string());
    }
    if let Some(e) = strict {
        a.insert(
            "strict_refusal".into(),
            cut(&e.summary(), INDEX_ATTR_VALUE_MAX),
        );
    }
    Ok(d)
}

/// A name or value as a key's id: printable ASCII but `/` and `%` as is, every other byte as
/// `%XX` (so distinct names stay distinct).
pub fn key_id(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if (0x21..=0x7e).contains(&b) && b != b'/' && b != b'%' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `@<kind>:<id>`, or `None` over the kit's key length (the kit mints one).
fn key(kind: &str, id: &str) -> Option<String> {
    let k = format!("@{kind}:{id}");
    (k.len() <= INDEX_KEY_MAX).then_some(k)
}

/// Each definition's key id, in number order: `defs` maps a number to its definition (an
/// aperture's or a tool's); the first of a definition is its id as is ([`key_id`]), and a second,
/// third… with the same definition is `~2`, `~3`, in number order.
fn ids_by_definition<N: Copy + Ord>(defs: &BTreeMap<N, String>) -> BTreeMap<N, String> {
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    let mut ids = BTreeMap::new();
    for (num, def) in defs {
        let n = seen.entry(def.as_str()).or_insert(0);
        *n += 1;
        let id = key_id(def);
        ids.insert(*num, if *n == 1 { id } else { format!("{id}~{n}") });
    }
    ids
}

/// An aperture's definition, but a block's: its template (a macro by name), its parameters,
/// and its `.AperFunction` (`circle,0.25;ViaPad`).
fn aperture_definition(ap: &photoplot::Aperture) -> String {
    let mut s = ap.template.name().to_owned();
    if let Template::Macro(name) = &ap.template {
        s.push('.');
        s.push_str(name);
    }
    for p in &ap.params {
        s.push(',');
        s.push_str(&p.to_string());
    }
    if let Some(f) = ap.attrs.iter().find(|a| a.name == ".AperFunction") {
        s.push(';');
        s.push_str(&f.value());
    }
    s
}

/// An object as its digests hold it: what it is and what it means, never a byte offset, an
/// aperture number or a list position. Its aperture by `ids` (`region` for a region); a
/// region's contours by their points; its transform and attributes by their values; the block
/// that holds it by `ids`, unless `in_block` (a block's own digest).
fn object_record(
    m: &Photoplot,
    o: &photoplot::Object,
    ids: &BTreeMap<u32, String>,
    in_block: bool,
) -> String {
    let shape = match &o.shape {
        Shape::Region { contours } => {
            let mut s = String::from("region");
            for ci in contours.start..contours.end {
                let c = &m.contours[ci as usize];
                let segs = &m.segments[c.segments.start as usize..c.segments.end as usize];
                s.push_str(&format!("[{:?}{segs:?}]", c.start));
            }
            s
        }
        other => format!("{other:?}"),
    };
    let ap = if o.aperture == 0 {
        "region"
    } else {
        ids.get(&o.aperture).map_or("?", String::as_str)
    };
    let block = if in_block || o.block == 0 {
        ""
    } else {
        ids.get(&o.block).map_or("?", String::as_str)
    };
    format!(
        "{shape};{:?};{ap};{:?};{:?};{:?};{block};{}",
        o.polarity,
        m.transforms.get(o.transform as usize),
        m.object_attr_sets.get(o.attrs as usize),
        m.region_attr_sets.get(o.region_attrs as usize),
        o.step_repeat
    )
}

/// Each aperture's key id (`@ap:<id>`), by number: its definition (its template, a macro by
/// name, its parameters and its `.AperFunction`), or for a block aperture `block,<16 hex>`, a
/// digest of what it holds (its objects as the digests hold them, an aperture inside by its
/// definition, so a nested block first); identical definitions `~2`, `~3` in number order. A
/// re-export that renumbers the apertures keeps every key.
pub fn aperture_ids(m: &Photoplot) -> BTreeMap<u32, String> {
    if mutant::active("KeyByNumber") {
        // planted: the number, which a re-export reissues
        return m
            .apertures
            .iter()
            .map(|a| (a.number, a.number.to_string()))
            .collect();
    }
    let mut defs: BTreeMap<u32, String> = BTreeMap::new();
    let mut blocks = Vec::new();
    for ap in &m.apertures {
        if ap.template == Template::Block {
            blocks.push(ap.number);
        } else {
            defs.insert(ap.number, aperture_definition(ap));
        }
    }
    // a block's digest needs the definitions of the apertures inside it, a nested block's too
    while !blocks.is_empty() {
        let before = blocks.len();
        let mut still = Vec::new();
        for b in blocks {
            let inner: Vec<&photoplot::Object> =
                m.objects.iter().filter(|o| o.block == b).collect();
            if inner
                .iter()
                .all(|o| o.aperture == 0 || o.aperture == b || defs.contains_key(&o.aperture))
            {
                let mut d = Digest::stream();
                for o in &inner {
                    d.update(object_record(m, o, &defs, true).as_bytes());
                    d.update(b"\n");
                }
                let hex: String = d.finish().as_bytes()[..8]
                    .iter()
                    .map(|x| format!("{x:02x}"))
                    .collect();
                defs.insert(b, format!("block,{hex}"));
            } else {
                still.push(b);
            }
        }
        if still.len() == before {
            // a block whose contents name an aperture never defined: by its number
            for b in still {
                defs.insert(b, format!("block,number-{b}"));
            }
            break;
        }
        blocks = still;
    }
    ids_by_definition(&defs)
}

/// Each tool's key id (`@tool:<id>`), by number: its diameter and its `.AperFunction`
/// (`0.3;Plated,PTH,ViaDrill`), identical ones `~2`, `~3` in number order. A re-export that
/// renumbers the tools keeps every key.
pub fn tool_ids(m: &Drill) -> BTreeMap<u8, String> {
    if mutant::active("KeyByNumber") {
        // planted: the number, which a re-export reissues
        return m
            .tools
            .iter()
            .map(|t| (t.number, t.number.to_string()))
            .collect();
    }
    let defs: BTreeMap<u8, String> = m
        .tools
        .iter()
        .map(|t| {
            let mut s = t.diameter.to_string();
            if let Some(f) = t.attrs.iter().find(|a| a.name == ".AperFunction") {
                s.push(';');
                s.push_str(&f.value());
            }
            (t.number, s)
        })
        .collect();
    ids_by_definition(&defs)
}

fn cut(s: &str, max: usize) -> String {
    cut_text(s, max as u32).0.to_owned()
}

fn rect(b: &Bounds) -> Option<NodeRect> {
    b.b.map(|(l, t, r, bo)| NodeRect {
        left: l,
        top: t,
        right: r,
        bottom: bo,
    })
}

/// Builds the node list with the kit's limits in view.
struct Nodes {
    nodes: Vec<DraftNode>,
    text_max: u32,
}

impl Nodes {
    fn push(
        &mut self,
        kind: &str,
        name: &str,
        key: Option<String>,
        parent: Option<u32>,
        record: Digest,
    ) -> u32 {
        let mut n = DraftNode::new(kind, cut(name, INDEX_NAME_MAX), record);
        n.key = key;
        n.parent = parent;
        self.nodes.push(n);
        (self.nodes.len() - 1) as u32
    }

    fn last(&mut self) -> &mut DraftNode {
        self.nodes.last_mut().expect("a node was pushed")
    }

    fn attr(&mut self, i: u32, k: &str, v: impl ToString) {
        let v = cut(&v.to_string(), INDEX_ATTR_VALUE_MAX);
        self.nodes[i as usize].attrs.insert(k.to_owned(), v);
    }

    fn text(&mut self, i: u32, t: &str) {
        if t.is_empty() {
            return;
        }
        let (s, cut) = cut_text(t, self.text_max);
        let n = &mut self.nodes[i as usize];
        n.text = Some(s.to_owned());
        if cut {
            n.flags.insert("text_truncated".to_owned());
        }
    }

    fn family(&mut self, name: &str) -> u32 {
        self.push(
            "family",
            name,
            key("family", name),
            None,
            Digest::of(name.as_bytes()),
        )
    }
}

/// Lines of `name,value` for a node's text.
fn attr_lines(attrs: &[Attr]) -> String {
    attrs
        .iter()
        .map(|a| {
            if a.fields.is_empty() {
                a.name.clone()
            } else {
                format!("{},{}", a.name, a.value())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Objects grouped by attribute (domain letter, name, value).
#[derive(Default)]
struct AttrGroups {
    groups: BTreeMap<(char, String, String), Group>,
}

#[derive(Default)]
struct Group {
    count: u64,
    bounds: Bounds,
    digest: Option<DigestStream>,
}

impl Group {
    fn add(&mut self, b: &Bounds, record: &str) {
        self.count += 1;
        self.bounds.union(b);
        self.digest
            .get_or_insert_with(Digest::stream)
            .update(record.as_bytes());
    }
}

impl AttrGroups {
    fn add(&mut self, domain: char, attrs: &[Attr], b: &Bounds, record: &str) {
        for a in attrs {
            self.groups
                .entry((domain, a.name.clone(), a.value()))
                .or_default()
                .add(b, record);
        }
    }

    /// The `attributes` family: a node per name, a child per value.
    fn emit(self, n: &mut Nodes) {
        if self.groups.is_empty() {
            return;
        }
        let fam = n.family("attributes");
        let mut by_name: BTreeMap<(char, String), Vec<(String, Group)>> = BTreeMap::new();
        for ((d, name, value), g) in self.groups {
            by_name.entry((d, name)).or_default().push((value, g));
        }
        for ((d, name), values) in by_name {
            let id = format!("{d}.{}", key_id(&name));
            let total: u64 = values.iter().map(|(_, g)| g.count).sum();
            let mut bounds = Bounds::default();
            for (_, g) in &values {
                bounds.union(&g.bounds);
            }
            let i = n.push(
                "attribute",
                &name,
                key("attr", &id),
                Some(fam),
                Digest::of(format!("{d}{name}").as_bytes()),
            );
            n.attr(i, "domain", if d == 'o' { "object" } else { "aperture" });
            n.attr(i, "values", values.len());
            n.attr(i, "objects", total);
            n.last().bounds = rect(&bounds);
            for (value, g) in values {
                let j = n.push(
                    "value",
                    &value,
                    key("attrval", &format!("{id}={}", key_id(&value))),
                    Some(i),
                    Digest::of(format!("{d}{name}={value}").as_bytes()),
                );
                n.attr(j, "objects", g.count);
                n.text(j, &value);
                let node = n.last();
                node.bounds = rect(&g.bounds);
                node.content = g.digest.map(DigestStream::finish);
            }
        }
    }
}

fn file_attributes(n: &mut Nodes, attrs: &[Attr]) {
    if attrs.is_empty() {
        return;
    }
    let fam = n.family("file-attributes");
    for a in attrs {
        let i = n.push(
            "file-attribute",
            &a.name,
            key("tf", &key_id(&a.name)),
            Some(fam),
            Digest::of(format!("{a:?}").as_bytes()),
        );
        n.text(i, &a.value());
    }
}

fn check_count(n: &Nodes, limits: &IndexLimits) -> Result<(), IndexRefused> {
    if n.nodes.len() as u64 > u64::from(limits.max_nodes) {
        return Err(IndexRefused::TooManyNodes {
            count: n.nodes.len() as u64,
            limit: limits.max_nodes,
        });
    }
    Ok(())
}

fn warn_attrs(d: &mut IndexDraft, w: &crate::error::Warnings) {
    d.attrs
        .insert("conformance".into(), w.conformance().name().into());
    d.attrs.insert("warnings".into(), w.total().to_string());
    for (code, count) in &w.counts {
        let k = match code {
            WarnCode::Deprecated(x) => format!("warn.deprecated.{}", x.name()),
            WarnCode::UnknownCommand => "warn.unknown-command".to_owned(),
        };
        d.attrs.insert(k, count.to_string());
    }
}

/// An object's point bounds (its contours' for a region).
fn object_bounds(m: &Photoplot, s: &Shape) -> Bounds {
    let mut b = Bounds::default();
    match s {
        Shape::Draw { from, to } => {
            b.add(*from);
            b.add(*to);
        }
        Shape::Arc {
            from,
            to,
            center,
            ccw,
        } => b.add_arc(*from, *to, *center, *ccw, from == to),
        Shape::Flash { at } => b.add(*at),
        Shape::Region { contours } => {
            for c in &m.contours[contours.start as usize..contours.end as usize] {
                let mut p: Pt = c.start;
                b.add(p);
                for s in &m.segments[c.segments.start as usize..c.segments.end as usize] {
                    match s {
                        Segment::Line { to } => b.add(*to),
                        Segment::Arc { to, center, ccw } => {
                            b.add_arc(p, *to, *center, *ccw, p == *to)
                        }
                    }
                    p = s.end();
                }
            }
        }
    }
    b
}

#[derive(Default)]
struct ApStats {
    draws: u64,
    arcs: u64,
    flashes: u64,
    dark: u64,
    clear: u64,
    in_blocks: u64,
    in_step_repeats: u64,
    bounds: Bounds,
    digest: Option<DigestStream>,
}

/// Indexes a photoplot model.
pub fn index_photoplot(m: &Photoplot, limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    let mut d = IndexDraft::new("photoplot-2026.05");
    let mut n = Nodes {
        nodes: Vec::new(),
        text_max: limits.max_text_bytes,
    };
    // one pass over the objects
    let mut per_ap: BTreeMap<u32, ApStats> = BTreeMap::new();
    let mut regions = ApStats::default();
    let (mut contours, mut segments) = (0u64, 0u64);
    let mut groups = AttrGroups::default();
    let ap_attrs: BTreeMap<u32, &[Attr]> = m
        .apertures
        .iter()
        .map(|a| (a.number, a.attrs.as_slice()))
        .collect();
    let mut all = Bounds::default();
    let (mut draws, mut arcs, mut flashes, mut nregions, mut dark, mut clear) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let ids = aperture_ids(m);
    let id_of = |num: u32| ids.get(&num).cloned().unwrap_or_else(|| num.to_string());
    for o in &m.objects {
        let b = object_bounds(m, &o.shape);
        let record = object_record(m, o, &ids, false);
        let st = if let Shape::Region { contours: c } = &o.shape {
            nregions += 1;
            contours += u64::from(c.end - c.start);
            for ci in c.start..c.end {
                let r = &m.contours[ci as usize].segments;
                segments += u64::from(r.end - r.start);
            }
            groups.add(
                'a',
                &m.region_attr_sets[o.region_attrs as usize],
                &b,
                &record,
            );
            &mut regions
        } else {
            match o.shape {
                Shape::Draw { .. } => draws += 1,
                Shape::Arc { .. } => arcs += 1,
                _ => flashes += 1,
            }
            groups.add(
                'a',
                ap_attrs.get(&o.aperture).copied().unwrap_or(&[]),
                &b,
                &record,
            );
            per_ap.entry(o.aperture).or_default()
        };
        groups.add('o', &m.object_attr_sets[o.attrs as usize], &b, &record);
        match o.shape {
            Shape::Draw { .. } => st.draws += 1,
            Shape::Arc { .. } => st.arcs += 1,
            Shape::Flash { .. } => st.flashes += 1,
            Shape::Region { .. } => {}
        }
        if o.polarity == photoplot::Polarity::Dark {
            st.dark += 1;
            dark += 1;
        } else {
            st.clear += 1;
            clear += 1;
        }
        if o.block != 0 {
            st.in_blocks += 1;
        }
        if o.step_repeat != 0 {
            st.in_step_repeats += 1;
        }
        st.bounds.union(&b);
        if !mutant::active("BoundsSkipped") {
            all.union(&b);
        }
        st.digest
            .get_or_insert_with(Digest::stream)
            .update(record.as_bytes());
    }

    // doc attributes
    let a = &mut d.attrs;
    a.insert("format".into(), "photoplot".into());
    a.insert("spec".into(), "2026.05".into());
    a.insert("unit".into(), m.unit.name().into());
    a.insert("int_digits".into(), m.format.int_digits.to_string());
    a.insert("decimals".into(), m.format.decimals.to_string());
    a.insert(
        "zero_omission".into(),
        if m.format.trailing_zeros_omitted {
            "trailing"
        } else {
            "leading"
        }
        .into(),
    );
    a.insert("y_axis".into(), "up".into());
    a.insert(
        "polarity".into(),
        match m.file_polarity() {
            Some("Negative") => "negative",
            Some(_) => "positive",
            None => "unstated",
        }
        .into(),
    );
    if m.image_negative {
        a.insert("image_negative".into(), "true".into());
    }
    a.insert("apertures".into(), m.apertures.len().to_string());
    a.insert("macros".into(), m.macros.len().to_string());
    a.insert("objects".into(), m.objects.len().to_string());
    a.insert("expanded_objects".into(), m.expanded_objects.to_string());
    a.insert("draws".into(), draws.to_string());
    a.insert("arcs".into(), arcs.to_string());
    a.insert("flashes".into(), flashes.to_string());
    a.insert("regions".into(), nregions.to_string());
    a.insert("dark".into(), dark.to_string());
    a.insert("clear".into(), clear.to_string());
    a.insert("step_repeats".into(), m.step_repeats.len().to_string());
    if let Some((x0, y0, x1, y1)) = all.b {
        a.insert("bounds".into(), format!("{x0},{y0},{x1},{y1}"));
    }
    warn_attrs(&mut d, &m.warnings);

    // apertures
    // in key order, so a re-export that renumbers or reorders them moves nothing
    let fam = n.family("apertures");
    let mut macro_uses: BTreeMap<&str, u64> = BTreeMap::new();
    let mut by_id: Vec<(String, &photoplot::Aperture)> =
        m.apertures.iter().map(|a| (id_of(a.number), a)).collect();
    by_id.sort_by(|a, b| a.0.cmp(&b.0));
    for (id, ap) in by_id {
        if let Template::Macro(name) = &ap.template {
            *macro_uses.entry(name.as_str()).or_insert(0) += 1;
        }
        let i = n.push(
            "aperture",
            &format!("D{}", ap.number),
            key("ap", &id),
            Some(fam),
            Digest::of(format!("{:?}{:?}{:?}", ap.template, ap.params, ap.attrs).as_bytes()),
        );
        n.attr(i, "number", ap.number);
        n.attr(i, "template", ap.template.name());
        if let Template::Macro(name) = &ap.template {
            n.attr(i, "macro", name);
        }
        if !ap.params.is_empty() {
            let p: Vec<String> = ap.params.iter().map(|d| d.to_string()).collect();
            n.attr(i, "params", p.join(","));
        }
        if ap.template == Template::Block {
            n.attr(i, "block_objects", ap.block_objects);
            n.attr(i, "block_expanded", ap.block_expanded);
        }
        if let Some(f) = ap.attrs.iter().find(|a| a.name == ".AperFunction") {
            n.attr(i, "function", f.value());
        }
        let s = per_ap.get(&ap.number);
        let uses = s.map_or(0, |s| s.draws + s.arcs + s.flashes);
        n.attr(i, "objects", uses);
        n.text(i, &attr_lines(&ap.attrs));
        if let Some(s) = s {
            n.last().bounds = rect(&s.bounds);
        }
    }

    // macros
    if !m.macros.is_empty() {
        let fam = n.family("macros");
        for mac in &m.macros {
            let i = n.push(
                "macro",
                &mac.name,
                key("macro", &key_id(&mac.name)),
                Some(fam),
                Digest::of(format!("{:?}", mac.body).as_bytes()),
            );
            let codes: Vec<String> = mac
                .primitive_codes()
                .iter()
                .map(|c| c.to_string())
                .collect();
            n.attr(i, "primitives", codes.join(","));
            n.attr(i, "items", mac.body.len());
            n.attr(
                i,
                "apertures",
                macro_uses.get(mac.name.as_str()).copied().unwrap_or(0),
            );
        }
    }

    // objects, by aperture, then the regions
    let fam = n.family("objects");
    let stats = |n: &mut Nodes, i: u32, s: &ApStats| {
        n.attr(i, "draws", s.draws);
        n.attr(i, "arcs", s.arcs);
        n.attr(i, "flashes", s.flashes);
        n.attr(i, "dark", s.dark);
        n.attr(i, "clear", s.clear);
        n.attr(i, "in_blocks", s.in_blocks);
        n.attr(i, "in_step_repeats", s.in_step_repeats);
    };
    let mut groups_by_id: Vec<(String, u32, ApStats)> = per_ap
        .into_iter()
        .map(|(num, s)| (id_of(num), num, s))
        .collect();
    groups_by_id.sort_by(|a, b| a.0.cmp(&b.0));
    for (id, num, s) in groups_by_id {
        let i = n.push(
            "objects",
            &format!("D{num}"),
            key("objects", &id),
            Some(fam),
            Digest::of(format!("objects with {id}").as_bytes()),
        );
        n.attr(i, "aperture", num);
        stats(&mut n, i, &s);
        let node = n.last();
        node.bounds = rect(&s.bounds);
        node.content = s.digest.map(DigestStream::finish);
    }
    if nregions > 0 {
        let i = n.push(
            "regions",
            "regions",
            key("objects", "regions"),
            Some(fam),
            Digest::of(b"regions"),
        );
        n.attr(i, "regions", nregions);
        n.attr(i, "contours", contours);
        n.attr(i, "segments", segments);
        stats(&mut n, i, &regions);
        let node = n.last();
        node.bounds = rect(&regions.bounds);
        node.content = regions.digest.map(DigestStream::finish);
    }

    groups.emit(&mut n);

    if !m.step_repeats.is_empty() {
        let fam = n.family("step-repeats");
        for (k, sr) in m.step_repeats.iter().enumerate() {
            let ord = k + 1;
            let i = n.push(
                "step-repeat",
                &format!("step and repeat {ord}"),
                key("sr", &ord.to_string()),
                Some(fam),
                Digest::of(
                    format!("{}x{} {} {} {}", sr.x, sr.y, sr.i, sr.j, sr.objects).as_bytes(),
                ),
            );
            n.attr(i, "x", sr.x);
            n.attr(i, "y", sr.y);
            n.attr(i, "i", sr.i);
            n.attr(i, "j", sr.j);
            n.attr(i, "objects", sr.objects);
        }
    }

    file_attributes(&mut n, &m.file_attrs);
    check_count(&n, limits)?;
    d.nodes = n.nodes;
    Ok(d)
}

/// Indexes a drill model.
pub fn index_drill(m: &Drill, limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    let mut d = IndexDraft::new("drill-xnc-2021.11");
    let mut n = Nodes {
        nodes: Vec::new(),
        text_max: limits.max_text_bytes,
    };
    let mut groups = AttrGroups::default();
    let tool_attrs: BTreeMap<u8, &[Attr]> = m
        .tools
        .iter()
        .map(|t| (t.number, t.attrs.as_slice()))
        .collect();
    #[derive(Default)]
    struct T {
        hits: u64,
        hit_bounds: Bounds,
        hit_digest: Option<DigestStream>,
        routs: u64,
        segments: u64,
        rout_bounds: Bounds,
        rout_digest: Option<DigestStream>,
    }
    let mut per: BTreeMap<u8, T> = BTreeMap::new();
    let mut all = Bounds::default();
    let ids = tool_ids(m);
    let id_of = |num: u8| ids.get(&num).cloned().unwrap_or_else(|| num.to_string());
    // a hit or a rout as its digests hold it: its points, its tool by key and its attributes
    // by value, never a tool number, a list position or a byte offset
    let hole_attrs = |i: u32| m.hole_attr_sets.get(i as usize);
    for h in &m.hits {
        let mut b = Bounds::default();
        b.add(h.at);
        let record = format!("{:?};{};{:?}", h.at, id_of(h.tool), hole_attrs(h.attrs));
        let t = per.entry(h.tool).or_default();
        t.hits += 1;
        t.hit_bounds.union(&b);
        t.hit_digest
            .get_or_insert_with(Digest::stream)
            .update(record.as_bytes());
        all.union(&b);
        groups.add(
            'a',
            tool_attrs.get(&h.tool).copied().unwrap_or(&[]),
            &b,
            &record,
        );
        groups.add('o', &m.hole_attr_sets[h.attrs as usize], &b, &record);
    }
    for r in &m.routs {
        let mut b = Bounds::default();
        let mut p = r.start;
        b.add(p);
        let segs = &m.segments[r.segments.start as usize..r.segments.end as usize];
        for s in segs {
            match s {
                RoutSegment::Line { to } => b.add(*to),
                RoutSegment::Arc { to, .. } => b.add_minor_arc_hull(p, *to),
            }
            p = s.end();
        }
        let record = format!(
            "{:?};{};{segs:?};{:?}",
            r.start,
            id_of(r.tool),
            hole_attrs(r.attrs)
        );
        let t = per.entry(r.tool).or_default();
        t.routs += 1;
        t.segments += segs.len() as u64;
        t.rout_bounds.union(&b);
        t.rout_digest
            .get_or_insert_with(Digest::stream)
            .update(record.as_bytes());
        all.union(&b);
        groups.add(
            'a',
            tool_attrs.get(&r.tool).copied().unwrap_or(&[]),
            &b,
            &record,
        );
        groups.add('o', &m.hole_attr_sets[r.attrs as usize], &b, &record);
    }

    let a = &mut d.attrs;
    a.insert("format".into(), "drill".into());
    a.insert("spec".into(), "xnc-2021.11".into());
    a.insert("unit".into(), m.unit.name().into());
    a.insert("decimals".into(), drill::DECIMALS.to_string());
    a.insert("y_axis".into(), "up".into());
    a.insert("tools".into(), m.tools.len().to_string());
    a.insert("hits".into(), m.hits.len().to_string());
    a.insert("routs".into(), m.routs.len().to_string());
    a.insert("rout_segments".into(), m.segments.len().to_string());
    a.insert(
        "coords_without_point".into(),
        m.coords_without_point.to_string(),
    );
    a.insert("conformance".into(), "current".into());
    if let Some((x0, y0, x1, y1)) = all.b {
        a.insert("bounds".into(), format!("{x0},{y0},{x1},{y1}"));
    }

    // in key order, so a re-export that renumbers or reorders them moves nothing
    let fam = n.family("tools");
    let mut by_id: Vec<(String, &drill::Tool)> =
        m.tools.iter().map(|t| (id_of(t.number), t)).collect();
    by_id.sort_by(|a, b| a.0.cmp(&b.0));
    for (id, t) in by_id {
        let s = per.remove(&t.number).unwrap_or_default();
        let i = n.push(
            "tool",
            &format!("T{:02}", t.number),
            key("tool", &id),
            Some(fam),
            Digest::of(format!("{:?}{:?}", t.diameter, t.attrs).as_bytes()),
        );
        n.attr(i, "number", t.number);
        n.attr(i, "diameter", t.diameter);
        n.attr(i, "hits", s.hits);
        n.attr(i, "routs", s.routs);
        if let Some(f) = t.attrs.iter().find(|a| a.name == ".AperFunction") {
            n.attr(i, "function", f.value());
        }
        n.text(i, &attr_lines(&t.attrs));
        let mut b = s.hit_bounds;
        b.union(&s.rout_bounds);
        n.last().bounds = rect(&b);
        if s.hits > 0 {
            let j = n.push(
                "hits",
                &format!("hits T{:02}", t.number),
                key("hits", &id),
                Some(i),
                Digest::of(format!("hits {id}").as_bytes()),
            );
            n.attr(j, "hits", s.hits);
            let node = n.last();
            node.bounds = rect(&s.hit_bounds);
            node.content = s.hit_digest.map(DigestStream::finish);
        }
        if s.routs > 0 {
            let j = n.push(
                "routs",
                &format!("routs T{:02}", t.number),
                key("routs", &id),
                Some(i),
                Digest::of(format!("routs {id}").as_bytes()),
            );
            n.attr(j, "routs", s.routs);
            n.attr(j, "segments", s.segments);
            let node = n.last();
            node.bounds = rect(&s.rout_bounds);
            node.content = s.rout_digest.map(DigestStream::finish);
        }
    }
    groups.emit(&mut n);
    file_attributes(&mut n, &m.file_attrs);
    check_count(&n, limits)?;
    d.nodes = n.nodes;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_ids_escape_what_the_grammar_reserves() {
        assert_eq!(key_id("GND"), "GND");
        assert_eq!(key_id("a b/c%"), "a%20b%2Fc%25");
        assert_eq!(key_id("\u{e9}"), "%C3%A9");
        assert!(key("attrval", &"x".repeat(300)).is_none());
    }
}
