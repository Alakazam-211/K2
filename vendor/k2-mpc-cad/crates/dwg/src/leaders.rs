//! Leaders (`LEADER`: its points, path type and arrowhead flag, what it annotates and its
//! dimension style) and multileaders (`MULTILEADER`), the latter read into the drawing model's format-neutral leader content
//! ([`Mleader`], the type the DXF read fills too): the entity's fields with its annotation
//! context, which holds the content (text or a block) and the leaders.
//!
//! The context comes first, its leader roots before its content; then the entity's own fields.
//! Every handle sits in the handle stream in the order the fields name them: each leader line's
//! linetype and arrowhead (2010 and later), the content's text style or block, then the
//! entity's style, linetype, arrowhead, text style and block, its arrowheads and its block
//! labels' attribute definitions.
//!
//! What the model keeps matches the DXF read's: the text (when the content is text) with where
//! it sits and its attachment, the text height, each leader's landing (its connection point)
//! and lines (points in world x and y), the entity's arrowhead block and size. Lines and points
//! past [`LINES_MAX`] and [`POINTS_MAX`] are counted, not kept.
//!
//! From the specification's sections on the multileader entity and its annotation context, as
//! facts; our own code.

use k2_mpc_cad_model::error::CadError;
use k2_mpc_cad_model::leader::{Branch, Mleader, LINES_MAX, POINTS_MAX};

use crate::entities::{bad, counted};
use crate::frame::{Obj, Ver};
use crate::mutant;

/// A multileader's fields: the model's content, and what the drawing doesn't draw by.
#[derive(Debug, Clone, PartialEq)]
pub struct MleaderData {
    pub content: Mleader,
    /// The content is text, or a block (neither: no content).
    pub has_text: bool,
    pub has_block: bool,
    /// Its multileader style, its text's style and its content block, by handle.
    pub style: Option<u64>,
    pub text_style: Option<u64>,
    pub block: Option<u64>,
    /// The leader lines with their own handles (2010 and later), the entity's arrowheads and
    /// its block labels: how many handles each holds in the handle stream.
    pub(crate) line_handles: usize,
    pub(crate) arrowheads: usize,
    pub(crate) labels: usize,
    /// The context's arrowhead size (the entity's own is the model's).
    pub context_arrow: f64,
}

/// The fewest bits a leader root, a line, a point pair and a label take: their counts are
/// refused before anything is read when the data can't hold them.
const ROOT_BITS: usize = 34;
const LINE_BITS: usize = 6;
const PAIR_BITS: usize = 12;
const LABEL_BITS: usize = 6;

/// The model's lines and points kept so far, with the DXF read's bounds: a line past
/// [`LINES_MAX`] is counted once and its points aren't kept; a point past [`POINTS_MAX`] on a
/// kept line is counted.
#[derive(Default)]
struct Kept {
    lines: usize,
    points: usize,
    line_kept: bool,
}

impl Kept {
    fn line(&mut self, m: &mut Mleader) {
        self.line_kept = false;
        let Some(b) = m.leaders.last_mut() else {
            return;
        };
        if self.lines < LINES_MAX || mutant::active("MleaderBoundsUncounted") {
            b.lines.push(Vec::new());
            self.lines += 1;
            self.line_kept = true;
        } else {
            m.dropped += 1;
        }
    }

    fn point(&mut self, m: &mut Mleader, p: [f64; 2]) {
        if !self.line_kept {
            return;
        }
        if self.points < POINTS_MAX {
            if let Some(l) = m.leaders.last_mut().and_then(|b| b.lines.last_mut()) {
                l.push(p);
                self.points += 1;
            }
        } else {
            m.dropped += 1;
        }
    }
}

/// A multileader's fields, from its first field to its data's end (its handles after, by
/// [`mleader_handles`]).
pub(crate) fn mleader(o: &mut Obj<'_>) -> Result<MleaderData, CadError> {
    if o.ver >= Ver::R2010 {
        // the entity's version
        o.d.bs()?;
    }
    let mut m = Mleader::default();
    let mut kept = Kept::default();
    let mut line_handles = 0usize;
    // the context: its leader roots
    let n = o.d.bl()?;
    let roots = counted(o, n, ROOT_BITS)?;
    for _ in 0..roots {
        o.d.b()?;
        o.d.b()?;
        let connection = o.d.bd3()?;
        let direction = o.d.bd3()?;
        let landing = if mutant::active("MleaderLandingFromDirection") {
            direction
        } else {
            connection
        };
        m.leaders.push(Branch {
            landing: Some([landing[0], landing[1]]),
            lines: Vec::new(),
        });
        let n = o.d.bl()?;
        for _ in 0..counted(o, n, PAIR_BITS)? {
            o.d.bd3()?;
            o.d.bd3()?;
        }
        // its index and landing distance
        o.d.bl()?;
        o.d.bd()?;
        let n = o.d.bl()?;
        for _ in 0..counted(o, n, LINE_BITS)? {
            kept.line(&mut m);
            let n = o.d.bl()?;
            for _ in 0..counted(o, n, 6)? {
                let p = o.d.bd3()?;
                kept.point(&mut m, [p[0], p[1]]);
            }
            // its breaks: each a segment index and its start and end points
            let n = o.d.bl()?;
            for _ in 0..counted(o, n, 4)? {
                o.d.bl()?;
                let k = o.d.bl()?;
                for _ in 0..counted(o, k, PAIR_BITS)? {
                    o.d.bd3()?;
                    o.d.bd3()?;
                }
            }
            // its index
            o.d.bl()?;
            if o.ver >= Ver::R2010 {
                // its type, colour, weight, arrow size and override flags (its linetype and
                // arrowhead in the handle stream)
                o.d.bs()?;
                o.cmc()?;
                o.d.bl()?;
                o.d.bd()?;
                o.d.bl()?;
                line_handles += 1;
            }
        }
        if o.ver >= Ver::R2010 {
            // its attachment direction
            o.d.bs()?;
        }
    }
    // the context's content: scale, base point, text height, arrowhead size, landing gap,
    // attachments and alignment
    o.d.bd()?;
    o.d.bd3()?;
    let height = o.d.bd()?;
    let context_arrow = o.d.bd()?;
    o.d.bd()?;
    for _ in 0..4 {
        o.d.bs()?;
    }
    let has_text = o.d.b()?;
    let mut has_block = false;
    if has_text {
        let text = o.tv()?;
        let normal = o.d.bd3()?;
        let location = o.d.bd3()?;
        // direction, rotation, boundary width and height, line spacing factor and style,
        // colour
        o.d.bd3()?;
        for _ in 0..4 {
            o.d.bd()?;
        }
        o.d.bs()?;
        o.cmc()?;
        let attachment = o.d.bs()?;
        // flow direction, background colour, scale, transparency and flags
        o.d.bs()?;
        o.cmc()?;
        o.d.bd()?;
        o.d.bl()?;
        o.d.b()?;
        o.d.b()?;
        // columns: type, auto height, width, gutter, flow reversed, sizes
        o.d.bs()?;
        o.d.b()?;
        o.d.bd()?;
        o.d.bd()?;
        o.d.b()?;
        let n = o.d.bl()?;
        for _ in 0..counted(o, n, 2)? {
            o.d.bd()?;
        }
        // word break, and a flag the specification doesn't name
        o.d.b()?;
        o.d.b()?;
        let at = if mutant::active("MleaderAtFromNormal") {
            normal
        } else {
            location
        };
        m.text = Some(text);
        m.at = Some([at[0], at[1]]);
        m.attachment = Some(i64::from(attachment));
    } else {
        has_block = o.d.b()?;
        if has_block {
            // normal, location, scale, rotation, colour, then its transformation matrix
            o.d.bd3()?;
            o.d.bd3()?;
            o.d.bd3()?;
            o.d.bd()?;
            o.cmc()?;
            for _ in 0..16 {
                o.d.bd()?;
            }
        }
    }
    m.height = Some(height);
    // the context's plane: base point, direction, vertical, normal reversed
    o.d.bd3()?;
    o.d.bd3()?;
    o.d.bd3()?;
    o.d.b()?;
    if o.ver >= Ver::R2010 {
        // top and bottom attachments
        o.d.bs()?;
        o.d.bs()?;
    }
    // the entity's own: override flags, leader type, colour, weight, landing and dog-leg flags,
    // landing distance
    o.d.bl()?;
    o.d.bs()?;
    o.cmc()?;
    o.d.bl()?;
    o.d.b()?;
    o.d.b()?;
    o.d.bd()?;
    let arrow_size = o.d.bd()?;
    // content type, left and right attachments, angle type, alignment, text colour, frame,
    // block colour, scale, rotation and connection, annotative
    o.d.bs()?;
    for _ in 0..4 {
        o.d.bs()?;
    }
    o.cmc()?;
    o.d.b()?;
    o.cmc()?;
    o.d.bd3()?;
    o.d.bd()?;
    o.d.bs()?;
    o.d.b()?;
    // arrowheads (each a default flag, its block in the handle stream): up to 2007 only (the
    // specification marks them so; the 2010 to 2018 samples end exactly only without them)
    let mut arrowheads = 0;
    if o.ver < Ver::R2010 {
        let n = o.d.bl()?;
        arrowheads = counted(o, n, 1)?;
        for _ in 0..arrowheads {
            o.d.b()?;
        }
    }
    // block labels: text, index, width (each an attribute definition in the handle stream)
    let n = o.d.bl()?;
    let labels = counted(o, n, LABEL_BITS)?;
    for _ in 0..labels {
        o.tv()?;
        o.d.bs()?;
        o.d.bd()?;
    }
    // text direction negative, alignment in place, justification, scale factor
    o.d.b()?;
    o.d.bs()?;
    o.d.bs()?;
    o.d.bd()?;
    if o.ver >= Ver::R2010 {
        // attachment direction, top and bottom attachments
        for _ in 0..3 {
            o.d.bs()?;
        }
    }
    if o.ver >= Ver::R2013 {
        // leader extended to text
        o.d.b()?;
    }
    m.arrow_size = Some(if mutant::active("MleaderArrowFromContext") {
        context_arrow
    } else {
        arrow_size
    });
    Ok(MleaderData {
        content: m,
        has_text,
        has_block,
        style: None,
        text_style: None,
        block: None,
        line_handles,
        arrowheads,
        labels,
        context_arrow,
    })
}

/// A multileader's handles, after the entity's common ones.
pub(crate) fn mleader_handles(o: &mut Obj<'_>, m: &mut MleaderData) -> Result<(), CadError> {
    let total = m
        .line_handles
        .saturating_mul(2)
        .saturating_add(m.arrowheads)
        .saturating_add(m.labels)
        .saturating_add(6);
    if total.saturating_mul(8) > o.hs.left() {
        return Err(bad(
            "more handles than the handle stream holds",
            o.hs.pos() / 8,
        ));
    }
    if !mutant::active("MleaderLineHandlesSkipped") {
        // each leader line's linetype and arrowhead
        for _ in 0..m.line_handles {
            o.h()?;
            o.h()?;
        }
    }
    if m.has_text {
        m.text_style = o.h()?;
    } else if m.has_block {
        m.block = o.h()?;
    }
    m.style = o.h()?;
    // its leader linetype
    o.h()?;
    m.content.arrow_block = o.h()?;
    // its style's text style and block
    o.h()?;
    o.h()?;
    for _ in 0..m.arrowheads.saturating_add(m.labels) {
        o.h()?;
    }
    Ok(())
}

/// A leader's fields (`LEADER`).
#[derive(Debug, Clone, PartialEq)]
pub struct LeaderData {
    /// Its points, its arrow first, in world coordinates.
    pub points: Vec<[f64; 3]>,
    /// What it annotates: 0 text, 1 a tolerance, 2 a block reference, 3 nothing.
    pub annot_type: u16,
    /// 0 straight, 1 a spline.
    pub path_type: u16,
    pub n: [f64; 3],
    /// Its hook line is on its x direction.
    pub hook_x: bool,
    /// Its arrowhead is shown.
    pub arrow: bool,
    /// What it annotates and its dimension style, by handle.
    pub annotation: Option<u64>,
    pub dimstyle: Option<u64>,
}

/// A leader's fields (2004 and later), from its first field to its data's end (its handles
/// after, by [`leader_handles`]).
pub(crate) fn leader(o: &mut Obj<'_>) -> Result<LeaderData, CadError> {
    // a flag the specification finds always 0
    o.d.b()?;
    let annot_type = o.d.bs()?;
    let path_type = o.d.bs()?;
    let n = o.d.bl()?;
    let mut points = Vec::new();
    for _ in 0..counted(o, n, 6)? {
        points.push(o.d.bd3()?);
    }
    // its plane's origin, its extrusion, its x direction, the block offset, the end projection
    o.d.bd3()?;
    let n = o.d.bd3()?;
    for _ in 0..3 {
        o.d.bd3()?;
    }
    // the text box's height and width: up to 2007 only (the 2010 to 2018 samples' leaders end
    // exactly, with the flags the 2004 sample's has, only without them)
    if o.ver < Ver::R2010 {
        o.d.bd()?;
        o.d.bd()?;
    }
    let hook_x = o.d.b()?;
    let arrow = o.d.b()?;
    // three fields the specification doesn't name
    o.d.bs()?;
    o.d.b()?;
    o.d.b()?;
    Ok(LeaderData {
        points,
        annot_type,
        path_type,
        n,
        hook_x,
        arrow: if mutant::active("LeaderArrowFromHook") {
            hook_x
        } else {
            arrow
        },
        annotation: None,
        dimstyle: None,
    })
}

/// A leader's handles, after the entity's common ones: what it annotates, its dimension style.
pub(crate) fn leader_handles(o: &mut Obj<'_>, l: &mut LeaderData) -> Result<(), CadError> {
    let a = o.h()?;
    let d = o.h()?;
    (l.annotation, l.dimstyle) = if mutant::active("LeaderHandlesSwapped") {
        (d, a)
    } else {
        (a, d)
    };
    Ok(())
}
