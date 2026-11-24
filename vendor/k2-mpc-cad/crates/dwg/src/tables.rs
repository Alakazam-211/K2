//! The table objects a drawing's entities name: the control objects (each table's entries in
//! order), layers, linetypes, text styles, registered applications, dimension styles (a
//! leader's arrowhead settings), block records, and layouts.
//!
//! A table entry begins with the object common data, then its name (`TV`), its 64-flag (`B`),
//! before 2007 its external reference index plus one (`BS`), and its external-reference-
//! dependent flag (`B`); its handles begin with its owner, reactors, extension dictionary and
//! its external reference block.
//!
//! From the specification's chapter on object prescriptions (the control objects, LAYER,
//! LTYPE, STYLE, APPID, DIMSTYLE, BLOCK HEADER, LAYOUT), as facts; our own code.

use k2_mpc_cad_model::error::CadError;

use crate::bits::Colour;
use crate::error::BAD_VALUE;
use crate::frame::{object_common, Obj, ObjCommon, Ver};
use crate::mutant;

const NEXT: &str = "the file is damaged: open a copy the app that made it saves again";

fn bad(why: impl Into<String>, at: usize) -> CadError {
    CadError::new(BAD_VALUE, why, NEXT).at(at)
}

/// `n` handles from the handle stream, refused when it can't hold them (8 bits each at least)
/// before anything is allocated; null references left out.
fn handles(o: &mut Obj<'_>, n: usize) -> Result<Vec<u64>, CadError> {
    if n.saturating_mul(8) > o.hs.left() {
        return Err(bad(
            "more handles than the handle stream holds",
            o.hs.pos() / 8,
        ));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if let Some(h) = o.h()? {
            out.push(h);
        }
    }
    Ok(out)
}

/// A control object's entries, in order (then, for the block table, model space and paper
/// space; for the linetype table, BYLAYER and BYBLOCK: `extra` more).
pub fn control(o: &mut Obj<'_>, extra: usize) -> Result<Vec<u64>, CadError> {
    object_common(o)?;
    let n = o.d.bl()? as usize;
    // the dimension style table writes a byte more: a count of further handles after its
    // entries (the specification lists none; the 2004 to 2018 samples write it)
    let more = if o.kind == 0x44 {
        usize::from(o.d.rc()?)
    } else {
        0
    };
    o.finish()?;
    let mut out = handles(o, n.saturating_add(extra))?;
    out.truncate(n.saturating_add(extra));
    if more > 0 {
        handles(o, more)?;
    }
    Ok(out)
}

/// What every table entry has: its common object data, name and flags.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Entry {
    pub common: ObjCommon,
    pub name: String,
    /// Dependent on an external reference: before 2007, its own flag; from 2007, the short's
    /// bit 256 (the drawing read also takes it from a `|` in the name).
    pub xdep: bool,
    /// From 2007, the short after the name (`BS`; 0 in every entry of the public samples, 256
    /// in an entry an external reference brings); 0 before 2007.
    pub state: u16,
    pub xref_block: Option<u64>,
}

fn entry(o: &mut Obj<'_>) -> Result<Entry, CadError> {
    let common = object_common(o)?;
    let name = o.tv()?;
    // before 2007: the 64-flag, the external reference index plus one, the dependent flag; from
    // 2007 one short in their place (as two bits it matches only the 2-bit forms of the short)
    let (state, xdep) = if !o.ver.strings() {
        o.d.b()?;
        o.d.bs()?;
        (0, o.d.b()?)
    } else if mutant::active("EntryStateAsTwoBits") {
        o.d.b()?;
        (0, o.d.b()?)
    } else if mutant::active("EntryXrefIndexAlways") {
        o.d.b()?;
        o.d.bs()?;
        (0, o.d.b()?)
    } else {
        let state = o.d.bs()?;
        let dependent = state & 0x100 != 0 && !mutant::active("EntryDependentBitIgnored");
        (state, dependent)
    };
    Ok(Entry {
        common,
        name,
        xdep,
        state,
        xref_block: None,
    })
}

/// A layer.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LayerRec {
    pub entry: Entry,
    /// Frozen 1, off 2, frozen in new viewports 4, locked 8, plotted 16; the lineweight's
    /// index in bits 5 to 9.
    pub flags: u16,
    pub colour: Colour,
    pub ltype: Option<u64>,
}

impl LayerRec {
    pub fn frozen(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn off(&self) -> bool {
        self.flags & 2 != 0
    }
    pub fn locked(&self) -> bool {
        self.flags & 8 != 0
    }
    pub fn plotted(&self) -> bool {
        self.flags & 16 != 0
    }
    /// The lineweight's index ([`crate::frame::lineweight`]).
    pub fn lineweight(&self) -> u8 {
        ((self.flags >> 5) & 0x1F) as u8
    }
}

pub fn layer(o: &mut Obj<'_>) -> Result<LayerRec, CadError> {
    let mut entry = entry(o)?;
    let flags = o.d.bs()?;
    let colour = o.cmc()?;
    o.finish()?;
    entry.xref_block = o.h()?;
    o.h()?;
    if o.ver.strings() {
        o.h()?;
    }
    let ltype = o.h()?;
    // from 2013, its visual style (unused here): a layer without it is cut short
    if o.ver >= Ver::R2013 && !mutant::active("LayerVisualStyleSkipped") {
        o.h()?;
    }
    Ok(LayerRec {
        entry,
        flags,
        colour,
        ltype,
    })
}

/// One dash of a linetype, and its complex element when it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Dash {
    pub length: f64,
    pub shape: u16,
    pub offset: [f64; 2],
    pub scale: f64,
    /// Radians.
    pub rotation: f64,
    /// 2: a text; 4: a shape (the bits DXF files use: the specification's description of them
    /// is the other way round, but its own rule for the text area follows bit 2).
    pub flags: u16,
    pub text: Option<String>,
    pub style: Option<u64>,
}

/// A linetype.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LtypeRec {
    pub entry: Entry,
    pub description: String,
    pub length: f64,
    pub dashes: Vec<Dash>,
}

/// The 0-terminated text at byte `at` of a linetype's text area.
fn area_text(area: &[u8], at: usize, wide: bool) -> Option<String> {
    let rest = area.get(at..)?;
    if wide {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    } else {
        let bytes: String = rest
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| char::from(b))
            .collect();
        Some(crate::frame::cp1252(&bytes))
    }
}

pub fn ltype(o: &mut Obj<'_>) -> Result<LtypeRec, CadError> {
    let entry = entry(o)?;
    let description = o.tv()?;
    let length = o.d.bd()?;
    o.d.rc()?;
    let n = usize::from(o.d.rc()?);
    let mut dashes = Vec::with_capacity(n);
    for _ in 0..n {
        let length = o.d.bd()?;
        let shape = o.d.bs()?;
        let offset = o.d.rd2()?;
        let scale = o.d.bd()?;
        let rotation = o.d.bd()?;
        let flags = o.d.bs()?;
        dashes.push(Dash {
            length,
            shape,
            offset,
            scale,
            rotation,
            flags,
            text: None,
            style: None,
        });
    }
    let area_len = if !o.ver.strings() {
        256
    } else if dashes.iter().any(|d| d.flags & 2 != 0) {
        512
    } else {
        0
    };
    let mut area = Vec::with_capacity(area_len);
    for _ in 0..area_len {
        area.push(o.d.rc()?);
    }
    o.finish()?;
    let mut entry = entry;
    entry.xref_block = o.h()?;
    for d in &mut dashes {
        d.style = o.h()?;
        let text_bit = if mutant::active("LtypeTextBitSwapped") {
            4
        } else {
            2
        };
        if d.flags & text_bit != 0 {
            d.text = area_text(&area, usize::from(d.shape), o.ver.strings());
        }
    }
    Ok(LtypeRec {
        entry,
        description,
        length,
        dashes,
    })
}

/// A text style.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StyleRec {
    pub entry: Entry,
    pub vertical: bool,
    /// A shape file, not a font.
    pub shape_file: bool,
    pub fixed_height: f64,
    pub width: f64,
    /// Radians.
    pub oblique: f64,
    /// 2 backwards, 4 upside down.
    pub generation: u8,
    pub font: String,
    pub bigfont: String,
}

pub fn style(o: &mut Obj<'_>) -> Result<StyleRec, CadError> {
    let mut entry = entry(o)?;
    // the specification names these "vertical (1 bit of flag)" then "shape file (4 bit)"; the
    // flag bits they stand for are the other way round (1 a shape file, 4 vertical), and the
    // program's own DXF save of the 2018 sample agrees: a shape file first, then vertical
    let (mut shape_file, mut vertical) = (o.d.b()?, o.d.b()?);
    if mutant::active("StyleBitsSwapped") {
        std::mem::swap(&mut shape_file, &mut vertical);
    }
    let fixed_height = o.d.bd()?;
    let width = o.d.bd()?;
    let oblique = o.d.bd()?;
    let generation = o.d.rc()?;
    o.d.bd()?;
    let font = o.tv()?;
    let bigfont = o.tv()?;
    o.finish()?;
    entry.xref_block = o.h()?;
    Ok(StyleRec {
        entry,
        vertical,
        shape_file,
        fixed_height,
        width,
        oblique,
        generation,
        font,
        bigfont,
    })
}

/// A registered application: its name.
pub fn appid(o: &mut Obj<'_>) -> Result<Entry, CadError> {
    let mut e = entry(o)?;
    o.d.rc()?;
    o.finish()?;
    e.xref_block = o.h()?;
    Ok(e)
}

/// A dimension style's name alone.
pub fn dimstyle_name(o: &mut Obj<'_>) -> Result<String, CadError> {
    Ok(entry(o)?.name)
}

/// A dimension style: the settings a leader's arrowhead is drawn by (the rest are read past).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DimStyleRec {
    pub entry: Entry,
    /// `DIMSCALE`, the overall scale.
    pub scale: f64,
    /// `DIMASZ`, the arrow size before the overall scale.
    pub arrow_size: f64,
    /// `DIMLDRBLK`, a leader's arrowhead block, by its block record's handle.
    pub leader_arrow: Option<u64>,
}

/// A dimension style (2004 and later): every setting read in its order to the data's end, its
/// overall scale, arrow size and leader arrowhead block kept.
pub fn dimstyle(o: &mut Obj<'_>) -> Result<DimStyleRec, CadError> {
    let mut entry = entry(o)?;
    let since2007 = o.ver.strings();
    // DIMPOST, DIMAPOST
    o.tv()?;
    o.tv()?;
    let mut scale = o.d.bd()?;
    let mut arrow_size = o.d.bd()?;
    if mutant::active("DimScaleAsArrow") {
        std::mem::swap(&mut scale, &mut arrow_size);
    }
    // DIMEXO, DIMDLI, DIMEXE, DIMRND, DIMDLE, DIMTP, DIMTM
    for _ in 0..7 {
        o.d.bd()?;
    }
    if since2007 {
        // DIMFXL, DIMJOGANG, DIMTFILL, DIMTFILLCLR
        o.d.bd()?;
        o.d.bd()?;
        o.d.bs()?;
        o.cmc()?;
    }
    // DIMTOL, DIMLIM, DIMTIH, DIMTOH, DIMSE1, DIMSE2, then DIMTAD, DIMZIN, DIMAZIN
    for _ in 0..6 {
        o.d.b()?;
    }
    for _ in 0..3 {
        o.d.bs()?;
    }
    if since2007 {
        // DIMARCSYM
        o.d.bs()?;
    }
    // DIMTXT, DIMCEN, DIMTSZ, DIMALTF, DIMLFAC, DIMTVP, DIMTFAC, DIMGAP, DIMALTRND
    for _ in 0..9 {
        o.d.bd()?;
    }
    // DIMALT, DIMALTD, DIMTOFL, DIMSAH, DIMTIX, DIMSOXD
    o.d.b()?;
    o.d.bs()?;
    for _ in 0..4 {
        o.d.b()?;
    }
    // DIMCLRD, DIMCLRE, DIMCLRT: object colours (the specification lists them as shorts; the
    // samples' dimension styles end exactly, with the settings before and after them equal to
    // the program's own DXF save's, only as colours)
    for _ in 0..3 {
        if mutant::active("DimColoursAsShorts") {
            o.d.bs()?;
        } else {
            o.cmc()?;
        }
    }
    // DIMADEC, DIMDEC, DIMTDEC, DIMALTU, DIMALTTD, DIMAUNIT, DIMFRAC, DIMLUNIT, DIMDSEP,
    // DIMTMOVE, DIMJUST
    for _ in 0..11 {
        o.d.bs()?;
    }
    // DIMSD1, DIMSD2, DIMTOLJ, DIMTZIN, DIMALTZ, DIMALTTZ, DIMUPT, DIMFIT
    o.d.b()?;
    o.d.b()?;
    for _ in 0..4 {
        o.d.bs()?;
    }
    o.d.b()?;
    o.d.bs()?;
    if since2007 {
        // DIMFXLON
        o.d.b()?;
    }
    if o.ver >= Ver::R2010 {
        // DIMTXTDIRECTION, DIMALTMZF, DIMALTMZS, DIMMZF, DIMMZS
        o.d.b()?;
        o.d.bd()?;
        o.tv()?;
        o.d.bd()?;
        o.tv()?;
    }
    // DIMLWD, DIMLWE, a flag
    o.d.bs()?;
    o.d.bs()?;
    o.d.b()?;
    o.finish()?;
    entry.xref_block = o.h()?;
    // DIMTXSTY, then DIMLDRBLK
    o.h()?;
    let leader_arrow = o.h()?;
    // DIMBLK, DIMBLK1, DIMBLK2, and from 2007 DIMLTYPE, DIMLTEX1, DIMLTEX2 (unused here): a
    // style without them is cut short
    if !mutant::active("TrailingHandlesSkipped") {
        for _ in 0..if since2007 { 6 } else { 3 } {
            o.h()?;
        }
    }
    Ok(DimStyleRec {
        entry,
        scale,
        arrow_size,
        leader_arrow,
    })
}

/// A block record.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct BlockHeader {
    pub entry: Entry,
    pub anonymous: bool,
    pub has_attdefs: bool,
    pub xref: bool,
    pub overlay: bool,
    pub base: [f64; 3],
    pub xref_path: String,
    pub description: String,
    /// Its BLOCK and ENDBLK entities.
    pub block_entity: Option<u64>,
    pub endblk: Option<u64>,
    /// Its entities, in order (none for an external reference's record).
    pub entities: Vec<u64>,
    pub layout: Option<u64>,
}

pub fn block_header(o: &mut Obj<'_>) -> Result<BlockHeader, CadError> {
    let mut entry = entry(o)?;
    let anonymous = o.d.b()?;
    let has_attdefs = o.d.b()?;
    let xref = o.d.b()?;
    let overlay = o.d.b()?;
    o.d.b()?;
    // the count of its entities: an external reference's record (attached or overlaid) owns
    // none and writes no count (read as one, every later field is off and the record is lost)
    let owned = if (xref || overlay) && !mutant::active("XrefOwnedCountRead") {
        0
    } else {
        o.d.bl()? as usize
    };
    let base = o.d.bd3()?;
    let xref_path = o.tv()?;
    let mut inserts = 0usize;
    while o.d.rc()? != 0 {
        inserts += 1;
    }
    let description = o.tv()?;
    let preview = o.d.bl()? as usize;
    o.d.skip(preview.saturating_mul(8))?;
    if o.ver.strings() {
        o.d.bs()?;
        o.d.b()?;
        o.d.rc()?;
    }
    o.finish()?;
    entry.xref_block = o.h()?;
    let block_entity = o.h()?;
    let entities = if mutant::active("BlockOwnedUncounted") {
        Vec::new()
    } else {
        handles(o, owned)?
    };
    let endblk = o.h()?;
    if inserts.saturating_mul(8) > o.hs.left() {
        return Err(bad(
            "more references than the handle stream holds",
            o.hs.pos() / 8,
        ));
    }
    for _ in 0..inserts {
        o.h()?;
    }
    let layout = o.h()?;
    Ok(BlockHeader {
        entry,
        anonymous,
        has_attdefs,
        xref,
        overlay,
        base,
        xref_path,
        description,
        block_entity,
        endblk,
        entities,
        layout,
    })
}

/// A layout: its plot settings' paper, its name and tab, its block record and viewports.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LayoutRec {
    pub name: String,
    pub tab: u32,
    /// Left, bottom, right, top, in millimetres.
    pub margins: [f64; 4],
    pub paper: [f64; 2],
    /// Quarter turns.
    pub rotation: u16,
    pub block: Option<u64>,
    pub viewports: Vec<u64>,
}

pub fn layout(o: &mut Obj<'_>) -> Result<LayoutRec, CadError> {
    object_common(o)?;
    o.tv()?;
    o.tv()?;
    o.d.bs()?;
    let margins = [o.d.bd()?, o.d.bd()?, o.d.bd()?, o.d.bd()?];
    let paper = o.d.bd2()?;
    o.tv()?;
    o.d.bd2()?;
    o.d.bs()?;
    let rotation = o.d.bs()?;
    o.d.bs()?;
    o.d.bd2()?;
    o.d.bd2()?;
    o.d.bd()?;
    o.d.bd()?;
    o.tv()?;
    o.d.bs()?;
    o.d.bd()?;
    o.d.bd2()?;
    for _ in 0..3 {
        o.d.bs()?;
    }
    let name = o.tv()?;
    let tab = o.d.bl()?;
    o.d.bs()?;
    o.d.bd3()?;
    o.d.rd2()?;
    o.d.rd2()?;
    for _ in 0..3 {
        o.d.bd3()?;
    }
    o.d.bd()?;
    o.d.bs()?;
    o.d.bd3()?;
    o.d.bd3()?;
    // the viewport count: a `BL` (the specification gives an `RL`; the samples' layouts end
    // only so)
    let n = if mutant::active("LayoutCountRaw") {
        o.d.rl()? as usize
    } else {
        o.d.bl()? as usize
    };
    o.finish()?;
    o.h()?;
    if o.ver.strings() {
        o.h()?;
    }
    let block = o.h()?;
    for _ in 0..3 {
        o.h()?;
    }
    let viewports = handles(o, n)?;
    Ok(LayoutRec {
        name,
        tab,
        margins,
        paper,
        rotation,
        block,
        viewports,
    })
}

/// A draw-order table: the block record it orders, and each entity with its sort handle.
pub type SortTable = (Option<u64>, Vec<(u64, u64)>);

/// A draw-order table (the sort handles in the data stream, the entities in the handle stream).
pub fn sortents(o: &mut Obj<'_>) -> Result<SortTable, CadError> {
    object_common(o)?;
    let n = o.d.bl()? as usize;
    if n.saturating_mul(8) > o.d.left() {
        return Err(bad("more sort handles than the data holds", o.d.pos() / 8));
    }
    let mut sort = Vec::with_capacity(n);
    for _ in 0..n {
        sort.push(o.d.h()?.value);
    }
    o.finish()?;
    let block = o.h()?;
    let mut pairs = Vec::with_capacity(n);
    for s in sort {
        if let Some(e) = o.h()? {
            pairs.push((e, s));
        }
    }
    Ok((block, pairs))
}

/// A colour-book colour (a `DBCOLOR` object): its colour number, true colour, name and book.
#[derive(Debug, Clone, PartialEq)]
pub struct DbColor {
    pub index: u16,
    /// `0xRRGGBB`, when it is a true colour.
    pub rgb: Option<u32>,
    pub name: String,
    pub book: String,
}

impl DbColor {
    /// The name DXF files give it: `BOOK$NAME`.
    pub fn dxf_name(&self) -> String {
        format!("{}${}", self.book, self.name)
    }
}

/// A colour-book colour: the object common data, then one object colour (`CMC`: its number,
/// its true colour, its flags, then its name and its book's name when the flags say). The
/// specification gives the object no prescription of its own: the read must end at its data's
/// end.
/// An image definition: the path of its image file, as written (the file is never opened).
pub fn imagedef(o: &mut Obj<'_>) -> Result<String, CadError> {
    object_common(o)?;
    // its class version, size in pixels
    o.d.bl()?;
    o.d.rd2()?;
    let path = o.tv()?;
    // loaded, resolution units, a pixel's size
    o.d.b()?;
    o.d.rc()?;
    o.d.rd2()?;
    o.finish()?;
    Ok(path)
}

pub fn dbcolor(o: &mut Obj<'_>) -> Result<DbColor, CadError> {
    object_common(o)?;
    let index = o.d.bs()?;
    let rgb = o.d.bl()?;
    let flags = o.d.rc()?;
    let mut name = if flags & 1 != 0 {
        o.tv()?
    } else {
        String::new()
    };
    let mut book = if flags & 2 != 0 {
        o.tv()?
    } else {
        String::new()
    };
    if mutant::active("DbColorNamesSwapped") {
        std::mem::swap(&mut name, &mut book);
    }
    o.finish()?;
    Ok(DbColor {
        index,
        rgb: (rgb >> 24 == 0xC2).then_some(rgb & 0x00FF_FFFF),
        name,
        book,
    })
}

/// The bits an object's handle stream holds past the handles its kind's read takes, beyond the
/// last byte's padding: 0 when the read took them all; more is data a newer or another writer
/// added, which the reads tolerate and the drawing counts ([`crate::drawing::Tolerated`]).
pub fn extra_handle_bits(o: &Obj<'_>) -> usize {
    let left = o.hs.left();
    if left >= 8 {
        left
    } else {
        0
    }
}
