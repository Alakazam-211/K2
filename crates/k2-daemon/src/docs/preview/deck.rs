//! `.pptx` slides: the first [`super::PreviewRenderer`].
//!
//! - **Listing** (in the daemon): the pptx pack's view index
//!   (`k2_mpc_pptx::view::index`) over the bytes drawn. Each slide's digest
//!   is the pack's render closure (contract Item 1: the slide, its layout,
//!   master, theme, media, charts, table styles...; never the notes or the
//!   clock), so an edit on one slide leaves the others' digests as they were.
//!   The live pack already parses the deck in this process; the listing is
//!   bounded by the same package limits.
//! - **Drawing** (out of process): one `k2-deckrender` per render (no
//!   pool), with its own time, memory, file and network limits
//!   (`k2_deckrender::host`), WebP lossless at the width asked. The worker
//!   draws each date field's cached text and never reads the clock.
//!
//! **Which worker** (the k2-export order): `K2_DECKRENDER_BIN` in a debug
//! build only (tests), else `k2-deckrender` beside the daemon (`K2
//! Daemon.app`, packages). With neither, every render fails
//! `renderer_error` and the route says the renderer isn't installed.

use std::path::PathBuf;
use std::time::Duration;

use k2_core::log_debug;
use k2_deckrender::host::{self, Limits, Request};
use k2_deckrender::proto::{FailureCode, ImageFormat, SlideRef, BINARY_NAME, PROTO, VERSION};

use super::{Container, FailCode, Failed, Image, Listing, Page, PreviewRenderer, Source, Width};
use crate::docs::target::{Target, Tier};

/// Debug builds only: the worker binary.
pub const ENV_BIN: &str = "K2_DECKRENDER_BIN";

/// The worker's memory cap (its allocator's; the OS limits add headroom).
pub const MEMORY_BYTES: u64 = 1 << 30;

pub struct Deck;

pub static DECK: Deck = Deck;

/// The worker binary: the debug override, else beside this executable.
pub fn binary() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        if let Some(p) = std::env::var_os(ENV_BIN).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()) {
            return Some(p);
        }
    }
    let here = std::env::current_exe().ok()?;
    let beside = here.parent()?.join(BINARY_NAME);
    beside.is_file().then_some(beside)
}

/// `@sld:256` → 256.
pub fn sld_id(address: &str) -> Option<u32> {
    address.strip_prefix("@sld:").and_then(|n| n.parse().ok())
}

fn code(c: FailureCode) -> FailCode {
    match c {
        FailureCode::TooLarge => FailCode::TooLarge,
        FailureCode::Timeout => FailCode::Timeout,
        // A package part it can't draw, or bytes that aren't a deck: the file's.
        FailureCode::UnsupportedPart | FailureCode::InvalidInput => FailCode::UnsupportedPart,
        // No such slide (the listing said there was one) or the worker itself.
        FailureCode::RendererError | FailureCode::NoSuchSlide => FailCode::RendererError,
    }
}

impl PreviewRenderer for Deck {
    fn kind(&self) -> &'static str {
        "deck"
    }

    fn build(&self) -> String {
        format!("deckrender-p{PROTO}-{VERSION}")
    }

    /// A deck served live by the Slides core (`tier_now` is `Deck` only
    /// while that core is on).
    fn serves(&self, t: &Target) -> bool {
        t.tier == Tier::Deck
    }

    fn list(&self, bytes: &[u8]) -> Result<Listing, Failed> {
        let ix = k2_mpc_pptx::view::index(bytes, &k2_mpc_pptx::Limits::default())
            .map_err(|d| Failed::new(FailCode::UnsupportedPart, format!("the deck's index: {d}")))?;
        let containers = ix
            .containers
            .iter()
            .filter_map(|c| {
                Some(Container {
                    locator: sld_id(&c.key)?.to_string(),
                    key: c.key.clone(),
                    position: c.position,
                    title: c.title.clone(),
                    hidden: c.hidden,
                    digest: *c.render_hash.as_bytes(),
                    extras: serde_json::Map::new(),
                })
            })
            .collect();
        Ok(Listing { page: Some(Page { w: ix.page.0, h: ix.page.1, unit: "emu" }), containers })
    }

    /// Always opaque (the route refuses a transparent deck preview), on the
    /// slide's own page (a slide isn't shaped: the shape asked is ignored).
    fn render(&self, src: &Source<'_>, container: &Container, width: Width, _look: super::Look, _shape: super::Shape, wall: Duration) -> Result<Image, Failed> {
        let Some(bin) = binary() else {
            return Err(Failed::new(FailCode::RendererError, format!("{BINARY_NAME} isn't installed beside the daemon")));
        };
        let limits = Limits {
            wall,
            memory_bytes: MEMORY_BYTES,
            max_input_bytes: crate::docs::blob_max_bytes(),
            max_output_bytes: width.max_bytes(),
            max_side: 8192,
            max_pixels: 32 << 20,
            threads: 0,
        };
        let Ok(sld) = container.locator.parse::<u32>() else {
            return Err(Failed::new(FailCode::RendererError, format!("not a slide id: {:?}", container.locator)));
        };
        let mut req = Request::new(src.doc, SlideRef::SldId(sld), width.px());
        req.format = ImageFormat::Webp;
        let started = std::time::Instant::now();
        match host::render(&bin, &req, &limits) {
            Ok(r) => Ok(Image {
                mime: match r.format {
                    ImageFormat::Webp => "image/webp",
                    ImageFormat::Png => "image/png",
                },
                format: match r.format {
                    ImageFormat::Webp => "webp-lossless",
                    ImageFormat::Png => "png",
                },
                width: r.meta.width,
                height: r.meta.height,
                fonts_substituted: r.meta.fonts.substituted.iter().map(|s| (s.requested.clone(), s.used.clone())).collect(),
                without_modifiers: false,
                render_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
                extras: serde_json::Map::new(),
                ids: None,
                bytes: r.image,
            }),
            Err(f) => {
                if !f.stderr.is_empty() {
                    // Debug log only: it may quote the file.
                    log_debug!("[docs/preview] k2-deckrender stderr ({}): {}", f.code, f.stderr.chars().take(2_000).collect::<String>());
                }
                Err(Failed::new(code(f.code), f.detail))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_mpc_pptx::testgen::{deck, DeckSpec, Slide};

    #[test]
    fn a_deck_lists_its_slides_by_address_with_their_digests() {
        let bytes = deck(&DeckSpec::new(vec![Slide::titled(256, "Quarterly review", ""), Slide::titled(257, "Goals", "Grow the pilot")]));
        let l = DECK.list(&bytes).expect("a listing");
        let page = l.page.expect("a deck has a page");
        assert_eq!(page.unit, "emu");
        assert!(page.w > 0 && page.h > 0);
        let keys: Vec<(&str, u32, &str, &str)> =
            l.containers.iter().map(|c| (c.key.as_str(), c.position, c.locator.as_str(), c.title.as_str())).collect();
        assert_eq!(keys, [("@sld:256", 1, "256", "Quarterly review"), ("@sld:257", 2, "257", "Goals")]);
        assert_ne!(l.containers[0].digest, l.containers[1].digest, "two slides draw from different parts");
        assert_eq!(DECK.list(&bytes).expect("again"), l, "the same bytes list the same");
    }

    /// Editing one slide's text changes that slide's digest only (the
    /// render closure isn't transitive): only that slide re-renders.
    #[test]
    fn an_edit_dirties_only_its_slide() {
        let a = deck(&DeckSpec::new(vec![Slide::titled(256, "One", ""), Slide::titled(257, "Two", ""), Slide::titled(258, "Three", "")]));
        let b = deck(&DeckSpec::new(vec![Slide::titled(256, "One", ""), Slide::titled(257, "Two, revised", ""), Slide::titled(258, "Three", "")]));
        let (la, lb) = (DECK.list(&a).expect("a"), DECK.list(&b).expect("b"));
        let changed: Vec<&str> =
            la.containers.iter().zip(&lb.containers).filter(|(x, y)| x.digest != y.digest).map(|(x, _)| x.key.as_str()).collect();
        assert_eq!(changed, ["@sld:257"]);
    }

    #[test]
    fn junk_is_unsupported_and_failures_map_to_the_contracts_codes() {
        let e = DECK.list(b"not a zip").expect_err("junk");
        assert_eq!(e.code, FailCode::UnsupportedPart);
        assert_eq!(code(FailureCode::InvalidInput), FailCode::UnsupportedPart);
        assert_eq!(code(FailureCode::NoSuchSlide), FailCode::RendererError);
        assert_eq!(code(FailureCode::Timeout), FailCode::Timeout);
        assert_eq!(code(FailureCode::TooLarge), FailCode::TooLarge);
        assert_eq!(sld_id("@sld:300"), Some(300));
        assert_eq!(sld_id("@sld:300/sp:4"), None);
        assert_eq!(sld_id("s300"), None);
    }
}
