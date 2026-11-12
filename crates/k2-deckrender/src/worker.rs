//! The worker's side: read a `.pptx`, draw one slide, describe it. Runs in
//! the `k2-deckrender` process the host starts per render (and in-process in
//! the tests and the fuzz targets); every failure is a [`Failure`] with its
//! [`FailureCode`].
//!
//! [`open`] is the parse path (fuzzed): sniffing, deckcraft-pptx's import, the
//! file's embedded fonts, the slide list. [`render`] adds the slide lookup, the
//! size checks, the draw (bundled fonts only, see [`crate::fonts`]; date fields
//! from the request or the file, never the clock), the flattening onto white,
//! the encoding (WebP lossless or PNG), the shape boxes and the fonts.

use std::collections::BTreeSet;
use std::io::{Cursor, Read};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

use deckcraft_model::resolve::{self, Ctx};
use deckcraft_model::text::RunKind;
use deckcraft_model::{Presentation, Shape, ShapeKind, Xfrm};
use kurbo::{Affine, Rect};

use crate::proto::{FailureCode, FontSubstitution, Fonts, ImageFormat, Meta, ShapeBox, SlideRef, Timings, MAX_NAME_CHARS, MAX_SHAPES};

/// EMU per point.
const EMU_PER_PT: f64 = 12_700.0;

/// deckcraft's largest pixmap side (vello_cpu sizes are u16).
pub const RENDERER_MAX_SIDE: u32 = deckcraft_render::MAX_SIDE;

/// Largest package part this module reads itself (the slide list's parts).
const MAX_LIST_PART: u64 = 16 << 20;

/// Most embedded font files added from one package.
const MAX_EMBEDDED_FONTS: usize = 64;

/// A typed failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: FailureCode,
    pub detail: String,
}

impl Failure {
    pub fn new(code: FailureCode, detail: impl Into<String>) -> Failure {
        Failure { code, detail: detail.into() }
    }
}

/// A drawn slide.
pub struct Output {
    pub image: Vec<u8>,
    pub format: ImageFormat,
    pub meta: Meta,
}

/// What the worker checks and how it draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opts {
    /// Longest image side, in pixels (at most [`RENDERER_MAX_SIDE`]).
    pub max_side: u32,
    /// Most pixels in the image.
    pub max_pixels: u64,
    /// Render threads (0: this thread).
    pub threads: u16,
    pub format: ImageFormat,
    /// The text of every date field (`None`: the file's cached text).
    pub date_text: Option<String>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { max_side: 8192, max_pixels: 32 << 20, threads: 0, format: ImageFormat::Webp, date_text: None }
    }
}

/// A read package.
pub struct Opened {
    pub pres: Presentation,
    /// Slide parts the package lists but the reader left out.
    pub lost: Vec<String>,
    /// The sldId of each slide in `pres.slides`, when the package's slide list
    /// matches the reader's (`None`: it doesn't, or it couldn't be read).
    pub sld_ids: Option<Vec<u32>>,
    /// Faces added from the file's embedded fonts.
    pub embedded_faces: usize,
}

const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// The parse path: sniff the bytes, import the package, add its embedded fonts, match its slide
/// list. A panic inside the reader is a [`FailureCode::RendererError`], never a crash.
pub fn open(bytes: &[u8]) -> Result<Opened, Failure> {
    crate::fonts::init();
    if bytes.starts_with(&CFB_MAGIC) {
        return Err(if deckcraft_pptx::is_encrypted(bytes) {
            Failure::new(FailureCode::UnsupportedPart, "a password-protected presentation (an encrypted package)")
        } else {
            Failure::new(FailureCode::UnsupportedPart, "a compound file (a binary .ppt or similar), not a .pptx package")
        });
    }
    if !bytes.starts_with(b"PK") {
        return Err(Failure::new(FailureCode::InvalidInput, "not a .pptx package (no zip signature)"));
    }
    let imported = catch_unwind(AssertUnwindSafe(|| deckcraft_pptx::import_checked(bytes)))
        .map_err(|_| Failure::new(FailureCode::RendererError, "the .pptx reader panicked"))?;
    let (pres, lost) = imported.map_err(|e| match e {
        deckcraft_pptx::PptxError::NotPptx(m) => Failure::new(FailureCode::InvalidInput, format!("not a .pptx package: {m}")),
        deckcraft_pptx::PptxError::Xml(m) => Failure::new(FailureCode::InvalidInput, format!("malformed XML: {m}")),
        deckcraft_pptx::PptxError::Unsupported(m) => Failure::new(FailureCode::UnsupportedPart, format!("unsupported: {m}")),
        deckcraft_pptx::PptxError::Write(m) => Failure::new(FailureCode::RendererError, m),
    })?;
    let embedded_faces = catch_unwind(AssertUnwindSafe(|| add_embedded_fonts(&pres))).unwrap_or(0);
    let sld_ids = catch_unwind(AssertUnwindSafe(|| slide_ids(bytes, &lost)))
        .ok()
        .flatten()
        .filter(|ids| ids.len() == pres.slides.len());
    Ok(Opened { pres, lost, sld_ids, embedded_faces })
}

/// The file's own embedded fonts (`p:embeddedFontLst`), as the reader's host does: they come
/// with the file, so every host draws the same with them. A face the bundled fonts already have
/// (same family and style) is not replaced. Returns the faces added.
fn add_embedded_fonts(p: &Presentation) -> usize {
    let db = deckcraft_fonts::FontDb::global();
    let mut seen = BTreeSet::new();
    let mut added = 0;
    for font in &p.embedded_fonts {
        for id in font.faces().into_iter().flatten() {
            if seen.len() >= MAX_EMBEDDED_FONTS || !seen.insert(id) {
                continue;
            }
            let Some(item) = p.media(id) else { continue };
            let Some(sfnt) = deckcraft_fonts::embedded_sfnt(&item.data) else { continue };
            added += db.add_font(sfnt.to_vec());
        }
    }
    if added > 0 {
        crate::fonts::preload(db);
    }
    added
}

fn ms(since: Instant) -> f64 {
    (since.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

/// Draw slide `slide` of the `.pptx` in `bytes`, `width` pixels wide.
pub fn render(bytes: &[u8], slide: SlideRef, width: u32, opts: &Opts) -> Result<Output, Failure> {
    let loaded = crate::fonts::init();
    let t_parse = Instant::now();
    let opened = open(bytes)?;
    let parse_ms = ms(t_parse);
    let pres = &opened.pres;
    let count = pres.slides.len();
    if count == 0 {
        return Err(Failure::new(FailureCode::NoSuchSlide, "the presentation has no slides the reader could read"));
    }
    let index = match slide {
        SlideRef::Index(i) if (i as usize) < count => i as usize,
        SlideRef::Index(i) => return Err(Failure::new(FailureCode::NoSuchSlide, format!("no slide {i}: the presentation has {count}"))),
        SlideRef::SldId(id) => match &opened.sld_ids {
            Some(ids) => ids
                .iter()
                .position(|x| *x == id)
                .ok_or_else(|| Failure::new(FailureCode::NoSuchSlide, format!("no slide with sldId {id}")))?,
            None => return Err(Failure::new(FailureCode::NoSuchSlide, "the package's slide list could not be matched to its slides; ask by index")),
        },
    };
    let (w_pt, h_pt) = (pres.slide_size.width, pres.slide_size.height);
    if !(w_pt.is_finite() && h_pt.is_finite() && w_pt > 0.0 && h_pt > 0.0) {
        return Err(Failure::new(FailureCode::InvalidInput, "the slide size is not a positive size"));
    }
    let max_side = opts.max_side.clamp(1, RENDERER_MAX_SIDE);
    if width == 0 {
        return Err(Failure::new(FailureCode::InvalidInput, "the width is 0"));
    }
    let scale = width as f64 / w_pt;
    let height_f = (h_pt * scale).round().max(1.0);
    if width > max_side || !height_f.is_finite() || height_f > max_side as f64 {
        return Err(Failure::new(FailureCode::TooLarge, format!("the slide is {width} x {height_f} px; at most {max_side} a side")));
    }
    let height = height_f as u32;
    if width as u64 * height as u64 > opts.max_pixels {
        return Err(Failure::new(FailureCode::TooLarge, format!("the slide is {width} x {height} px; at most {} pixels", opts.max_pixels)));
    }
    // Date fields: the request's text, else the file's own cached text; the clock is never read
    // (`now` is fixed, and unused while `date_text` is set).
    let (date_text, date_text_from) = match &opts.date_text {
        Some(t) => (t.clone(), "request"),
        None => match cached_date_text(pres, index) {
            Some(t) => (t, "file"),
            None => (String::new(), "none"),
        },
    };
    let ropts = deckcraft_render::RenderOpts {
        scale,
        threads: opts.threads,
        size: Some((width, height)),
        date_text: Some(date_text.clone()),
        now: Some(deckcraft_text::datetime::DateTime::new(2000, 1, 1, 0, 0, 0)),
        ..Default::default()
    };
    let t_render = Instant::now();
    let panics = deckcraft_render::panics_caught();
    let drawn = catch_unwind(AssertUnwindSafe(|| deckcraft_render::render_slide(pres, index, &ropts)))
        .map_err(|_| Failure::new(FailureCode::RendererError, "the renderer panicked"))?;
    if deckcraft_render::panics_caught() != panics {
        return Err(Failure::new(FailureCode::RendererError, format!("the renderer panicked on slide {index}")));
    }
    if drawn.width != width || drawn.height != height {
        return Err(Failure::new(FailureCode::RendererError, format!("the renderer drew {} x {}, not {width} x {height}", drawn.width, drawn.height)));
    }
    let render_ms = ms(t_render);
    let t_encode = Instant::now();
    let rgb = flatten_onto_white(&drawn.pixels, width, height);
    drop(drawn);
    let image = encode(&rgb, width, height, opts.format).map_err(|e| Failure::new(FailureCode::RendererError, format!("encoding: {e}")))?;
    drop(rgb);
    let encode_ms = ms(t_encode);
    let (shapes, shapes_omitted) = catch_unwind(AssertUnwindSafe(|| shape_boxes(pres, index))).unwrap_or_default();
    let fonts = catch_unwind(AssertUnwindSafe(|| fonts(pres, index, &ropts))).unwrap_or_default();
    let meta = Meta {
        width,
        height,
        slide_index: index as u32,
        sld_id: opened.sld_ids.as_ref().and_then(|ids| ids.get(index).copied()),
        slide_count: count as u32,
        slide_size_emu: [emu(w_pt), emu(h_pt)],
        shapes,
        shapes_omitted,
        fonts,
        date_text,
        date_text_from: date_text_from.into(),
        timings: Timings {
            fonts_ms: (loaded.took.as_secs_f64() * 1e6).round() / 1e3,
            parse_ms,
            render_ms,
            encode_ms,
            families: loaded.families as u32,
            faces: loaded.faces as u32,
        },
        lost_slides: opened.lost.len() as u32,
    };
    Ok(Output { image, format: opts.format, meta })
}

/// Premultiplied RGBA over white, to RGB.
pub fn flatten_onto_white(premultiplied: &[u8], width: u32, height: u32) -> Vec<u8> {
    let n = width as usize * height as usize;
    let mut out = Vec::with_capacity(n * 3);
    for px in premultiplied.as_chunks::<4>().0.iter().take(n) {
        let a = px[3];
        for c in &px[..3] {
            out.push(c.saturating_add(255 - a));
        }
    }
    out.resize(n * 3, 255);
    out
}

/// RGB8 to WebP lossless (image-webp) or PNG.
pub fn encode(rgb: &[u8], width: u32, height: u32, format: ImageFormat) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let mut buf = Vec::new();
    let r = match format {
        ImageFormat::Webp => image::codecs::webp::WebPEncoder::new_lossless(&mut buf).write_image(rgb, width, height, image::ExtendedColorType::Rgb8),
        ImageFormat::Png => image::codecs::png::PngEncoder::new(&mut buf).write_image(rgb, width, height, image::ExtendedColorType::Rgb8),
    };
    r.map_err(|e| e.to_string())?;
    if buf.is_empty() {
        return Err("no bytes".into());
    }
    Ok(buf)
}

/// The text of the first date field on the slide, then on its layout, then on its master, as the
/// file saved it.
fn cached_date_text(pres: &Presentation, index: usize) -> Option<String> {
    let slide = pres.slides.get(index)?;
    let ctx = Ctx::for_slide(pres, slide)?;
    let mut lists: Vec<&[Shape]> = vec![slide.shapes.as_slice()];
    if let Some(l) = ctx.layout {
        lists.push(l.shapes.as_slice());
    }
    lists.push(ctx.master.shapes.as_slice());
    for shapes in lists {
        let mut found: Option<String> = None;
        deckcraft_model::walk(shapes, &mut |s, _| {
            if found.is_some() {
                return;
            }
            let Some(body) = &s.text else { return };
            for p in &body.paragraphs {
                for r in &p.runs {
                    if let RunKind::Field { field } = &r.kind {
                        if deckcraft_text::datetime::is_date_field(field) && found.is_none() {
                            found = Some(r.text.clone());
                        }
                    }
                }
            }
        });
        if found.is_some() {
            return found;
        }
    }
    None
}

fn emu(pt: f64) -> i64 {
    let v = (pt * EMU_PER_PT).round();
    if v.is_finite() {
        v.clamp(i64::MIN as f64, i64::MAX as f64) as i64
    } else {
        0
    }
}

fn kind_of(s: &Shape) -> &'static str {
    match &s.kind {
        ShapeKind::Shape => "shape",
        ShapeKind::Picture { .. } => "picture",
        ShapeKind::Group { .. } => "group",
        ShapeKind::Connector { .. } => "connector",
        ShapeKind::Table(_) => "table",
        ShapeKind::Chart(_) => "chart",
        ShapeKind::Media(_) => "media",
        ShapeKind::Ink { .. } => "ink",
        ShapeKind::Model3d { .. } => "model3d",
        _ => "other",
    }
}

/// A box on the page, `a:xfrm` style: centre, extents, rotation (degrees), flips.
#[derive(Clone, Copy, Debug)]
struct PageBox {
    cx: f64,
    cy: f64,
    w: f64,
    h: f64,
    rot: f64,
    fh: bool,
    fv: bool,
}

fn quarter_turns(rot: f64) -> Option<i64> {
    let q = rot / 90.0;
    ((q - q.round()).abs() < 1e-9).then(|| q.round() as i64)
}

/// The child's box on the page from its group's page box and child space: the child's centre
/// maps through the group (scale, flips, rotation); its extents scale by the group's x and y
/// scales, swapped when the child itself is turned 90° or 270°; rotations add (negated under one
/// group flip) and flips combine. `true`: the scales differ and the child's rotation isn't a
/// multiple of 90°, so deckcraft draws it sheared and the box is approximate.
fn compose(g: &PageBox, ch: &Xfrm, c: &Xfrm) -> (PageBox, bool) {
    let sx = if ch.w.abs() > 1e-9 { g.w / ch.w } else { 1.0 };
    let sy = if ch.h.abs() > 1e-9 { g.h / ch.h } else { 1.0 };
    let odd_quarter = quarter_turns(c.rot).is_some_and(|q| q.rem_euclid(2) == 1);
    let (w, h) = if odd_quarter { (c.w * sy, c.h * sx) } else { (c.w * sx, c.h * sy) };
    let px = g.cx - g.w / 2.0 + (c.x + c.w / 2.0 - ch.x) * sx;
    let py = g.cy - g.h / 2.0 + (c.y + c.h / 2.0 - ch.y) * sy;
    let (mut qx, mut qy) = (px - g.cx, py - g.cy);
    if g.fh {
        qx = -qx;
    }
    if g.fv {
        qy = -qy;
    }
    let (sin, cos) = g.rot.to_radians().sin_cos();
    let sign = if g.fh != g.fv { -1.0 } else { 1.0 };
    let b = PageBox { cx: g.cx + qx * cos - qy * sin, cy: g.cy + qx * sin + qy * cos, w, h, rot: g.rot + sign * c.rot, fh: g.fh != c.flip_h, fv: g.fv != c.flip_v };
    let uneven = (sx - sy).abs() > 1e-9 * sx.abs().max(sy.abs()).max(1.0);
    (b, uneven && quarter_turns(c.rot).is_none())
}

fn norm_deg(r: f64) -> f64 {
    let r = r % 360.0;
    if r < 0.0 {
        r + 360.0
    } else {
        r
    }
}

/// The slide's own shapes with their page boxes (as deckcraft-render's `place_slide` walks them),
/// and how many were left past [`MAX_SHAPES`].
pub fn shape_boxes(pres: &Presentation, index: usize) -> (Vec<ShapeBox>, u64) {
    let mut out = Vec::new();
    let mut omitted = 0u64;
    let Some(slide) = pres.slides.get(index) else { return (out, 0) };
    let Some(ctx) = Ctx::for_slide(pres, slide) else { return (out, 0) };
    walk(&ctx, &slide.shapes, None, Affine::IDENTITY, None, 0, &mut out, &mut omitted);
    (out, omitted)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    ctx: &Ctx,
    shapes: &[Shape],
    parent: Option<(PageBox, Xfrm, bool)>,
    pm: Affine,
    group: Option<usize>,
    depth: usize,
    out: &mut Vec<ShapeBox>,
    omitted: &mut u64,
) {
    if depth > 32 {
        return;
    }
    for s in shapes {
        if s.hidden {
            continue;
        }
        let x = resolve::xfrm(ctx, s);
        if !x.is_finite() {
            continue;
        }
        let (pb, approx) = match &parent {
            None => (PageBox { cx: x.x + x.w / 2.0, cy: x.y + x.h / 2.0, w: x.w, h: x.h, rot: x.rot, fh: x.flip_h, fv: x.flip_v }, false),
            Some((g, ch, inherited)) => {
                let (b, a) = compose(g, ch, &x);
                (b, a || *inherited)
            }
        };
        let m = pm * x.affine();
        let r = m.transform_rect_bbox(Rect::new(0.0, 0.0, x.w, x.h));
        let me = if out.len() < MAX_SHAPES {
            let rot = norm_deg(pb.rot);
            out.push(ShapeBox {
                id: s.id.0,
                name: s.name.chars().take(MAX_NAME_CHARS).collect(),
                parent: group,
                kind: kind_of(s).into(),
                x: emu(pb.cx - pb.w / 2.0),
                y: emu(pb.cy - pb.h / 2.0),
                w: emu(pb.w),
                h: emu(pb.h),
                rot: if rot.is_finite() { (rot * 1e6).round() / 1e6 } else { 0.0 },
                flip_h: pb.fh,
                flip_v: pb.fv,
                approx_box: approx,
                bbox: [emu(r.x0), emu(r.y0), emu(r.width()), emu(r.height())],
                affine: m.as_coeffs().map(|c| if c.is_finite() { c } else { 0.0 }),
            });
            Some(out.len() - 1)
        } else {
            *omitted += 1;
            None
        };
        if let ShapeKind::Group { children, child } = &s.kind {
            let ch = Rect::new(child.x, child.y, child.x + child.w, child.y + child.h);
            let gm = pm * deckcraft_geom::group_child_affine(&x, ch);
            walk(ctx, children, Some((pb, *child, approx)), gm, me, depth + 1, out, omitted);
        }
    }
}

/// The families the slide was drawn in, and the Latin families its own runs
/// ask for that the bundled (and embedded) fonts lack, with what is drawn instead.
pub fn fonts(pres: &Presentation, index: usize, ropts: &deckcraft_render::RenderOpts) -> Fonts {
    let mut used = BTreeSet::new();
    for t in deckcraft_render::place_slide_with(pres, index, ropts).texts {
        for r in &t.layout.runs {
            used.insert(r.face.family.clone());
        }
    }
    let mut requested = BTreeSet::new();
    if let Some(slide) = pres.slides.get(index) {
        if let Some(ctx) = Ctx::for_slide(pres, slide) {
            deckcraft_model::walk(&slide.shapes, &mut |s, _| {
                let Some(body) = &s.text else { return };
                for p in &body.paragraphs {
                    for run in &p.runs {
                        let eff = resolve::run(&ctx, s, p, &run.props);
                        let fam = resolve::font_family(&ctx, &eff);
                        if !fam.is_empty() && requested.len() < 256 {
                            requested.insert(fam);
                        }
                    }
                }
            });
        }
    }
    let db = deckcraft_fonts::FontDb::global();
    let mut substituted = Vec::new();
    for fam in requested {
        if db.has_family(&fam) {
            continue;
        }
        let drawn = db.document_face(&fam, "Regular").family.clone();
        if !drawn.eq_ignore_ascii_case(&fam) {
            substituted.push(FontSubstitution { requested: fam, used: drawn });
        }
    }
    Fonts { used: used.into_iter().collect(), substituted }
}

// ── The package's slide list ─────────────────────────────────────────────

/// The sldId of each slide the reader kept, in its order: the presentation
/// part's `p:sldIdLst` resolved through its relationships, duplicates and
/// unresolved ids dropped as deckcraft-pptx drops them, and the parts it
/// couldn't read (`lost`) left out. `None` when the list can't be read.
pub fn slide_ids(bytes: &[u8], lost: &[String]) -> Option<Vec<u32>> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let root = read_part(&mut z, "_rels/.rels")?;
    let root = deckcraft_pptx::parse_xml(&root).ok()?;
    let main = root
        .root
        .children_named("Relationship")
        .find(|r| r.attr("Type").is_some_and(|t| t.rsplit('/').next() == Some("officeDocument")))
        .and_then(|r| r.attr("Target"))
        .map(|t| resolve_target("", t))
        .unwrap_or_else(|| "ppt/presentation.xml".to_string());
    let pres = deckcraft_pptx::parse_xml(&read_part(&mut z, &main)?).ok()?;
    let rels = deckcraft_pptx::parse_xml(&read_part(&mut z, &rels_path(&main))?).ok()?;
    let base = dir_of(&main);
    let mut by_id: Vec<(String, String)> = Vec::new();
    for r in rels.root.children_named("Relationship") {
        let (Some(id), Some(target)) = (r.attr("Id"), r.attr("Target")) else { continue };
        if r.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("external")) {
            continue;
        }
        if !by_id.iter().any(|(i, _)| i == id) {
            by_id.push((id.to_string(), resolve_target(base, target)));
        }
    }
    let lost: BTreeSet<String> = lost.iter().map(|p| norm(p)).collect();
    let mut seen = BTreeSet::new();
    let mut ids = Vec::new();
    let list = pres.root.child("sldIdLst")?;
    for s in list.children_named("sldId").take(10_000) {
        let rid = s.attr("r:id").unwrap_or("");
        let Some((_, target)) = by_id.iter().find(|(i, _)| i == rid) else { continue };
        let key = norm(target);
        if !seen.insert(key.clone()) || lost.contains(&key) {
            continue;
        }
        ids.push(s.u32("id").unwrap_or(0));
    }
    Some(ids)
}

/// A part's bytes (name matched as the reader matches it), at most
/// [`MAX_LIST_PART`].
fn read_part(z: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let want = norm(name);
    let index = (0..z.len().min(50_000)).find(|&i| z.name_for_index(i).and_then(Result::ok).is_some_and(|n| norm(&n) == want))?;
    let f = z.by_index(index).ok()?;
    let mut data = Vec::new();
    f.take(MAX_LIST_PART + 1).read_to_end(&mut data).ok()?;
    (data.len() as u64 <= MAX_LIST_PART).then_some(data)
}

fn norm(name: &str) -> String {
    name.trim_start_matches('/').replace('\\', "/").to_ascii_lowercase()
}

fn dir_of(part: &str) -> &str {
    part.rfind('/').map(|i| &part[..i]).unwrap_or("")
}

fn rels_path(part: &str) -> String {
    let p = part.trim_start_matches('/');
    match p.rfind('/') {
        Some(i) => format!("{}/_rels/{}.rels", &p[..i], &p[i + 1..]),
        None => format!("_rels/{p}.rels"),
    }
}

/// A relationship target against its source part's folder (`..`, `.`, a
/// leading `/`, `%XX` escapes and a `#fragment` handled as the reader does).
fn resolve_target(base: &str, target: &str) -> String {
    let target = percent_decode(target.split('#').next().unwrap_or("")).replace('\\', "/");
    let mut segs: Vec<&str> = if target.starts_with('/') { Vec::new() } else { base.split('/').filter(|s| !s.is_empty()).collect() };
    for s in target.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    segs.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if let Some(v) = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                if b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_resolve_as_the_reader_resolves_them() {
        assert_eq!(resolve_target("ppt", "slides/slide1.xml"), "ppt/slides/slide1.xml");
        assert_eq!(resolve_target("ppt", "/ppt/slides/slide2.xml"), "ppt/slides/slide2.xml");
        assert_eq!(resolve_target("ppt/slides", "../media/a%20b.png#x"), "ppt/media/a b.png");
        assert_eq!(resolve_target("ppt", "../../../x.xml"), "x.xml");
        assert_eq!(resolve_target("ppt", "a%+5b.xml"), "ppt/a%+5b.xml");
        assert_eq!(rels_path("ppt/presentation.xml"), "ppt/_rels/presentation.xml.rels");
        assert_eq!(rels_path("/root.xml"), "_rels/root.xml.rels");
    }

    #[test]
    fn flattening_puts_transparency_on_white() {
        // Opaque red, half-transparent black (premultiplied), fully transparent.
        let px = [255, 0, 0, 255, 0, 0, 0, 128, 0, 0, 0, 0];
        assert_eq!(flatten_onto_white(&px, 3, 1), vec![255, 0, 0, 127, 127, 127, 255, 255, 255]);
    }

    #[test]
    fn a_child_turned_90_in_an_uneven_group_swaps_scales_and_others_are_approximate() {
        let g = PageBox { cx: 100.0, cy: 50.0, w: 200.0, h: 100.0, rot: 0.0, fh: false, fv: false };
        let ch = Xfrm::new(0.0, 0.0, 100.0, 100.0);
        let mut c = Xfrm::new(25.0, 25.0, 50.0, 50.0);
        let (b, approx) = compose(&g, &ch, &c);
        assert_eq!((b.cx, b.cy, b.w, b.h, approx), (100.0, 50.0, 100.0, 50.0, false));
        c.rot = 90.0;
        let (b, approx) = compose(&g, &ch, &c);
        assert_eq!((b.w, b.h, b.rot, approx), (50.0, 100.0, 90.0, false));
        c.rot = 45.0;
        assert!(compose(&g, &ch, &c).1, "45 degrees in an uneven group is sheared");
    }

    #[test]
    fn non_packages_fail_typed() {
        assert_eq!(open(b"").err().map(|f| f.code), Some(FailureCode::InvalidInput));
        assert_eq!(open(b"hello world").err().map(|f| f.code), Some(FailureCode::InvalidInput));
        assert_eq!(open(b"PK\x03\x04truncated").err().map(|f| f.code), Some(FailureCode::InvalidInput));
        let mut cfb = CFB_MAGIC.to_vec();
        cfb.resize(4096, 0);
        assert_eq!(open(&cfb).err().map(|f| f.code), Some(FailureCode::UnsupportedPart));
    }
}
