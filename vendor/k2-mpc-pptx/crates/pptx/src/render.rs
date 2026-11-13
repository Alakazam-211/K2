//! The render hash (what a host's renderer reads): SHA-256 over one
//! slide's **render closure**, from the bytes the renderer draws. The kit keys the digest with
//! the space's key before anything stores it.
//!
//! **The closure follows only the edges named here** (it isn't transitive):
//!
//! - the slide part and its `.rels`;
//! - the slide's own relationship targets, with each one's `.rels`: its layout, images (media
//!   posters, OLE and SVG fallbacks), media, charts, diagrams and OLE objects;
//! - from a chart: its style, colours, images and theme override (never its workbook: the
//!   renderer draws the cached values); from a diagram's parts: their images;
//! - from the layout: its master (the part only: the master's `.rels` lists every layout) and
//!   the layout's own media;
//! - from the master: **only its theme and its media**, never master → layout;
//! - `tableStyles.xml`, for a slide with a table;
//! - from `presentation.xml`, only `p:sldSz` and `p:defaultTextStyle`;
//! - the slide's position, **only** for a slide with a slide-number field;
//! - the slide's fonts ([`crate::view::FontUse`]).
//!
//! **Never followed:** slide → slide relationships (hyperlink and action targets), the notes
//! slide, comments, tags, customXml, every other part, and the clock (the renderer draws a
//! date field's cached text, which is in the slide part).

use std::collections::BTreeSet;

use k2_mpc_base::index::Digest;
use k2_mpc_opc::name;

use crate::deck::{rels_of, Pptx, SlideRef};
use crate::mutant;
use crate::view::FontUse;
use crate::xmlx::{child, parse_xml, P};
use crate::{Decline, Limits};

/// The last segment of a relationship type.
fn kind(rel_type: &str) -> &str {
    rel_type.rsplit('/').next().unwrap_or("")
}

/// Media a part draws: images and the media types.
fn is_media(k: &str) -> bool {
    matches!(k, "image" | "video" | "audio" | "media" | "hdphoto")
}

struct Closure<'p, 'a> {
    p: &'p Pptx<'a>,
    limits: &'p Limits,
    parts: BTreeSet<String>,
}

impl Closure<'_, '_> {
    fn has(&self, key: &str) -> bool {
        self.p.opc.parts.contains_key(key)
    }

    fn add(&mut self, key: &str) {
        if self.has(key) {
            self.parts.insert(key.to_owned());
        }
    }

    /// The part and its relationships part.
    fn add_with_rels(&mut self, key: &str) {
        self.add(key);
        let rels = name::key(&name::rels_part_for(&self.p.name_of(key)));
        self.add(&rels);
    }

    /// The internal targets of `key`'s relationships, with their kind.
    fn targets(&self, key: &str) -> Result<Vec<(String, String)>, Decline> {
        let rels = rels_of(&self.p.opc, key, self.limits)?;
        let src = self.p.name_of(key);
        Ok(rels
            .list
            .iter()
            .filter(|r| !r.external)
            .filter_map(|r| {
                let t = r.target_key(&src)?;
                self.has(&t).then(|| (t, kind(&r.rel_type).to_owned()))
            })
            .collect())
    }

    fn walk(&mut self, slide: &SlideRef) -> Result<(), Decline> {
        self.add_with_rels(&slide.part);
        let overreach = mutant::active("HashMissesNothingButOverreaches");
        let mut layout = None;
        for (t, k) in self.targets(&slide.part)? {
            match k.as_str() {
                "slideLayout" => {
                    if !mutant::active("HashMissesLayout") {
                        self.add_with_rels(&t);
                    }
                    layout = Some(t);
                }
                "chart" => {
                    self.add_with_rels(&t);
                    for (c, ck) in self.targets(&t)? {
                        if matches!(
                            ck.as_str(),
                            "chartStyle" | "chartColorStyle" | "themeOverride"
                        ) || is_media(&ck)
                        {
                            self.add(&c);
                        }
                    }
                }
                "diagramData" | "diagramLayout" | "diagramQuickStyle" | "diagramColors"
                | "diagramDrawing" => {
                    self.add_with_rels(&t);
                    for (c, ck) in self.targets(&t)? {
                        if is_media(&ck) {
                            self.add(&c);
                        }
                    }
                }
                "oleObject" | "package" => self.add_with_rels(&t),
                "slide" if overreach => self.add_with_rels(&t),
                k if is_media(k) => self.add_with_rels(&t),
                _ => {}
            }
        }
        let Some(layout) = layout else { return Ok(()) };
        let mut master = None;
        for (t, k) in self.targets(&layout)? {
            if k == "slideMaster" {
                self.add(&t);
                master = Some(t);
            } else if is_media(&k) {
                self.add(&t);
            }
        }
        let Some(master) = master else { return Ok(()) };
        for (t, k) in self.targets(&master)? {
            if k == "theme" || is_media(&k) {
                self.add(&t);
            } else if k == "slideLayout" && overreach {
                self.add_with_rels(&t);
            }
        }
        Ok(())
    }
}

/// The raw digest of slide `slide`'s render closure: at `position` (from 1, in the hash only
/// when `slidenum`), with a table (`tableStyles.xml` in it) or not, drawing with `fonts`.
pub fn render_digest(
    p: &Pptx<'_>,
    slide: &SlideRef,
    position: usize,
    slidenum: bool,
    table: bool,
    fonts: &[FontUse],
    limits: &Limits,
) -> Result<Digest, Decline> {
    let mut c = Closure {
        p,
        limits,
        parts: BTreeSet::new(),
    };
    c.walk(slide)?;
    if table && !mutant::active("HashMissesTableStyle") {
        for (t, k) in c.targets(&p.main)? {
            if k == "tableStyles" {
                c.add(&t);
            }
        }
    }
    let mut d = Digest::stream();
    for key in &c.parts {
        let content = p.opc.content(key).unwrap_or(&[]);
        let n = p.name_of(key);
        d.update(b"part\0");
        d.update(n.as_bytes());
        d.update(b"\0");
        d.update(&(content.len() as u64).to_le_bytes());
        d.update(content);
    }
    let text = p.text(&p.main)?;
    let doc = parse_xml(text, limits)?;
    for e in ["sldSz", "defaultTextStyle"] {
        d.update(b"pres\0");
        d.update(e.as_bytes());
        d.update(b"\0");
        if let Some(n) = child(doc.root_element(), P, e) {
            let b = &text.as_bytes()[n.range()];
            d.update(&(b.len() as u64).to_le_bytes());
            d.update(b);
        }
    }
    if slidenum && !mutant::active("HashMissesSlideNumber") {
        d.update(format!("pos\0{position}\0").as_bytes());
    }
    for f in fonts {
        d.update(
            format!(
                "font\0{}\0{}\0{}\0{}\0",
                f.family, f.script, f.from, f.embedded
            )
            .as_bytes(),
        );
    }
    Ok(d.finish())
}
