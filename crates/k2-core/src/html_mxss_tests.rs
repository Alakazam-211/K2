//! Mutation-XSS checks for both ammonia cleaners: the ticket brief
//! (`feedback_brief`) and the mail reader (`mail_html`).
//!
//! Mutation XSS is markup that the cleaner reads one way and the browser
//! reads another once the cleaned copy is parsed again: most often a
//! MathML or SVG wrapper is removed, so a raw-text tag inside it (style,
//! iframe, xmp, noscript...) switches from foreign content to HTML and its
//! text becomes live tags. RUSTSEC-2026-0533 is that bug in ammonia before
//! 4.2.5 (a MathML text point holding SVG children without the `svg` root,
//! or SVG `foreignObject` holding MathML without `math`).
//!
//! Each cleaner's output is tokenized again as HTML and must hold only its
//! own allowlisted tags, no event-handler attribute, no `javascript:` value
//! and no comment. With no `svg`/`math` left, HTML is how a browser reads it.

use super::*;

/// The advisory's two shapes (from ammonia's own fix tests) plus the
/// older namespace-confusion bypasses.
const MXSS_INPUTS: &[&str] = &[
    "<math><mi><svg><iframe><!--</iframe><img src onerror=alert(1)>-->",
    "<svg><foreignObject><math><iframe><!--</iframe><img src onerror=alert(1)>-->",
    "<math><mi><svg><style><img src=x onerror=alert(1)></style></svg></mi></math>",
    "<math><mtext><svg><title><img src=x onerror=alert(1)></title></svg></mtext></math>",
    "<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>",
    "<form><math><mtext></form><form><mglyph><style></math><img src onerror=alert(1)>",
    "<svg></p><style><a id=\"</style><img src=1 onerror=alert(1)>\">",
    "<noscript><p title=\"</noscript><img src=x onerror=alert(1)>\">",
    "<math><annotation-xml encoding=\"text/html\"><style><img src=x onerror=alert(1)></style></annotation-xml></math>",
    "<svg><foreignObject><xmp></foreignObject><img src=x onerror=alert(1)></xmp></foreignObject></svg>",
];

/// Parse `out` again as HTML and fail on anything that could run.
fn assert_reparses_inert(input: &str, out: &str, allowed: &[&str]) {
    let mut problems = Vec::new();
    tokenize(out, |token| match token {
        TagToken(tag) => {
            let name = &*tag.name;
            if !allowed.contains(&name) {
                problems.push(format!("tag <{name}>"));
            }
            for attr in &tag.attrs {
                let attr_name = attr.name.local.to_ascii_lowercase();
                if attr_name.starts_with("on") {
                    problems.push(format!("event handler {attr_name} on <{name}>"));
                }
                if attr.value.to_ascii_lowercase().contains("javascript:") {
                    problems.push(format!("javascript: in {attr_name} on <{name}>"));
                }
            }
        }
        CommentToken(c) => problems.push(format!("comment {:?}", &*c)),
        _ => {}
    });
    assert!(problems.is_empty(), "input: {input}\ncleaned: {out}\nfound: {problems:?}");
}

/// The pinned ammonia has the RUSTSEC-2026-0533 fix: with math + mi and a
/// raw-text tag allowed but not svg (and the mirror case), the stray
/// foreign children go instead of turning into HTML. Fails on 4.2.4 and
/// older; guards the `=4.2.5` pin.
#[test]
fn the_ammonia_pin_carries_the_math_and_svg_mxss_fix() {
    let math = ammonia::Builder::default()
        .add_tags(&["math", "mi", "iframe"])
        .strip_comments(false)
        .clean(MXSS_INPUTS[0])
        .to_string();
    assert_eq!(math, "<math><mi></mi></math>");
    let svg = ammonia::Builder::default()
        .add_tags(&["svg", "foreignObject", "iframe"])
        .strip_comments(false)
        .clean(MXSS_INPUTS[1])
        .to_string();
    assert_eq!(svg, "<svg><foreignObject></foreignObject></svg>");
}

#[test]
fn the_brief_cleaner_leaves_nothing_live_from_mxss_inputs() {
    for input in MXSS_INPUTS {
        let out = sanitize(input);
        assert_reparses_inert(input, &out, ALLOWED_TAGS);
        assert_eq!(sanitize(&out), out, "cleaning twice changes nothing: {input}");
        assert!(removed_anything(input), "the removal is reported: {input}");
    }
}

#[test]
fn the_mail_cleaner_leaves_nothing_live_from_mxss_inputs() {
    for input in MXSS_INPUTS {
        let out = crate::mail_html::clean_mail_html(input).html;
        assert_reparses_inert(input, &out, crate::mail_html::MAIL_TAGS);
    }
}
