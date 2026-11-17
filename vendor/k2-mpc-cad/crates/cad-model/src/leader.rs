//! A multileader's content, format-neutral: its text with where it sits, its attachment and
//! height, each leader (its landing point and its lines, each line's points from its arrow),
//! and its arrowhead. A format's reader fills it (`k2_mpc_dxf::mleader` for DXF); the model
//! keeps its lines ([`crate::Extra::lines`]) and the drawing read draws it.

/// The leader lines a multileader keeps at most, over all its leaders; more are counted, not
/// kept.
pub const LINES_MAX: usize = 64;
/// The points a multileader keeps at most, over all its lines.
pub const POINTS_MAX: usize = 4_096;

/// One leader of a multileader.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Branch {
    /// Where its lines meet the landing (the point nearest the text).
    pub landing: Option<[f64; 2]>,
    /// Its lines, each one's points from its arrow, in world x and y.
    pub lines: Vec<Vec<[f64; 2]>>,
}

/// A multileader's content.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mleader {
    /// Its text as written, inline formatting codes kept ([`Mleader::plain`] drops them).
    pub text: Option<String>,
    /// Where its text sits.
    pub at: Option<[f64; 2]>,
    /// The text's attachment point (`1` top left … `9` bottom right), as written.
    pub attachment: Option<i64>,
    /// The text's height.
    pub height: Option<f64>,
    pub leaders: Vec<Branch>,
    /// Lines or points past [`LINES_MAX`] or [`POINTS_MAX`], not kept.
    pub dropped: u64,
    /// The arrowhead's block, by handle; `None` for the default (a closed filled arrow).
    pub arrow_block: Option<u64>,
    /// The arrowhead's size: the entity's own, else its context's.
    pub arrow_size: Option<f64>,
}

impl Mleader {
    /// Its first leader's first line from its arrow, then that leader's landing: its outline.
    pub fn first_line(&self) -> Vec<[f64; 2]> {
        let Some(b) = self.leaders.first() else {
            return Vec::new();
        };
        let mut v = b.lines.first().cloned().unwrap_or_default();
        v.extend(b.landing);
        v
    }

    /// Every leader line from its arrow, then its leader's landing.
    pub fn lines(&self) -> Vec<Vec<[f64; 2]>> {
        if crate::mutant::active("MleaderFirstLineOnly") {
            // the planted bug: the first line only, as the outline
            let first = self.first_line();
            return if first.is_empty() {
                Vec::new()
            } else {
                vec![first]
            };
        }
        self.leaders
            .iter()
            .flat_map(|b| {
                b.lines.iter().map(move |l| {
                    let mut v = l.clone();
                    v.extend(b.landing);
                    v
                })
            })
            .collect()
    }

    /// Its text without inline formatting codes, paragraphs on lines of their own.
    pub fn plain(&self) -> Option<String> {
        self.text.as_deref().map(plain)
    }
}

/// A multiline text's content without its inline formatting codes (fonts, colours, stacking,
/// grouping braces), its paragraphs on lines of their own: what it reads as.
pub fn plain(content: &str) -> String {
    crate::draw::text::parse_mtext(content, 1.0)
        .paras
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
