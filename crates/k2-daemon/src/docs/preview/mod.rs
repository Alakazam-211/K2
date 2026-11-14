//! Live previews: one host service, a renderer per doc type (rendering-data
//! contract r1.4, Item 8, gaps 17 and 26; K2's host markup §C).
//!
//! The **host** (this module) owns everything a doc type shares: which
//! bytes are drawn (the live head, `file_now`; a cut version, `file_at`),
//! the render hash keyed with the space's host cache key, the queue and its
//! slots, the host-side time cap, the sealed cache under
//! `~/.k2/cache/doc-previews/`, the route with the doc's rights, the
//! `preview_ready` push, and the five purge hooks. A **renderer**
//! ([`PreviewRenderer`]) owns only what is per type: how to list a file's
//! containers with the raw digest of what each draws from, and how to draw
//! one container at a width, out of process.
//!
//! | File | What |
//! |---|---|
//! | `mod.rs` | the per-type interface ([`PreviewRenderer`]), the registry ([`renderer_for`]), the shared types |
//! | [`deck`] | `.pptx` slides: the pptx pack's view index, drawn by `k2-deckrender` (one process per render) |
//! | [`scene`] | live BLEND scenes: the pack's mesh read (K2SCENE1; not in K2 yet), cameras and objects drawn by `k2-scenerender` (one process per request) |
//! | [`extras`] | what a listing and an image carry besides the image (a scene's bounds, cameras, turntable titles, id tables), in the client's shapes |
//! | [`reuse`] | a renamed container answered from the image its render hash already has, without a render |
//! | [`cache`] | the sealed on-disk cache, its cap, and the purge hooks' file side |
//! | [`service`] | per-doc index memo, dirty detection, the render queue and slots, the host time cap, `preview_ready` |
//! | [`route`] | `GET /cli/docs/preview` |
//!
//! **Adding a type** (K2 Cores' live plans r1: xlsx, timeline, BLEND):
//! implement [`PreviewRenderer`] for it and list it in [`renderers`]. What
//! a type previews is its own: a deck's slides; a workbook's sheet
//! thumbnails; a timeline's clip in-points and waveforms (their digest is
//! the media's content hash and the source time, the media read through
//! [`SideFiles`], never put on the wire); a scene's cameras or objects (the
//! digest over the meshes they draw). The host only needs, per previewed
//! thing, an address, an opaque locator and a raw digest. A type with no
//! page (a timeline) lists `page: None`. A host that can't draw a kind at
//! all (a timeline's video on a Linux host with no GPU: no software video
//! decoders) answers [`FailCode::NotOnThisHost`], which the view shows as a
//! "no preview" card rather than an error; a scene's stills come from K2
//! Render's 3D still renderer, a worker like `k2-deckrender`. The cache, the
//! route, the rights, the push and every purge hook come with the host.

pub mod cache;
pub mod deck;
pub mod extras;
pub mod reuse;
pub mod route;
pub mod scene;
pub mod service;

#[cfg(test)]
mod scene_tab_tests;

use super::target::Target;

/// A preview's width bucket (contract Q7): device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Width {
    /// 320 px: the filmstrip tile.
    Thumb,
    /// 640 px: a tile on a dense display (optional).
    Thumb2x,
    /// 1,920 px: the stage.
    Full,
}

impl Width {
    pub const ALL: [Width; 3] = [Width::Thumb, Width::Thumb2x, Width::Full];

    pub fn as_str(self) -> &'static str {
        match self {
            Width::Thumb => "thumb",
            Width::Thumb2x => "thumb2x",
            Width::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<Width> {
        Width::ALL.into_iter().find(|w| w.as_str() == s.trim())
    }

    pub fn px(self) -> u32 {
        match self {
            Width::Thumb => 320,
            Width::Thumb2x => 640,
            Width::Full => 1920,
        }
    }

    /// The largest image kept at this width (contract Item 8: a full image
    /// at most 4 MiB, under the docs socket's frame; a thumb far less).
    pub fn max_bytes(self) -> u64 {
        match self {
            Width::Thumb => 1 << 20,
            Width::Thumb2x => 2 << 20,
            Width::Full => 4 << 20,
        }
    }
}

/// How a still is drawn: on the renderer's own background, or transparent
/// (RGBA, coverage alpha, no background), so one image serves every look a
/// client paints behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Look {
    Opaque,
    Transparent,
}

impl Look {
    pub const ALL: [Look; 2] = [Look::Opaque, Look::Transparent];

    pub fn as_str(self) -> &'static str {
        match self {
            Look::Opaque => "opaque",
            Look::Transparent => "transparent",
        }
    }

    pub fn parse(s: &str) -> Option<Look> {
        Look::ALL.into_iter().find(|l| l.as_str() == s.trim())
    }

    /// The renderer build an image of this look is cached under: the build
    /// itself for an opaque image (the names every existing entry has),
    /// `<build>.rgba` for a transparent one. No background colour is in it:
    /// a transparent still has none.
    pub fn cache_build(self, build: &str) -> String {
        match self {
            Look::Opaque => build.to_string(),
            Look::Transparent => format!("{build}.rgba"),
        }
    }
}

/// Why a render failed (contract Item 8, typed; never file content).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailCode {
    TooLarge,
    UnsupportedPart,
    Timeout,
    RendererError,
    /// This host can't draw this kind at all (no decoder for it here): a
    /// "no preview" card, not an error. Never cached as a retry: the doc
    /// changing doesn't change the host. The deck renderer never answers it.
    NotOnThisHost,
}

impl FailCode {
    pub fn as_str(self) -> &'static str {
        match self {
            FailCode::TooLarge => "too_large",
            FailCode::UnsupportedPart => "unsupported_part",
            FailCode::Timeout => "timeout",
            FailCode::RendererError => "renderer_error",
            FailCode::NotOnThisHost => "not_on_this_host",
        }
    }
}

/// A failed render: the code is shown; `detail` goes to the debug log only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub code: FailCode,
    pub detail: String,
}

impl Failed {
    pub fn new(code: FailCode, detail: impl Into<String>) -> Failed {
        Failed { code, detail: detail.into() }
    }
}

/// A page's size in the file's own units (`emu` for a deck).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub w: i64,
    pub h: i64,
    pub unit: &'static str,
}

/// One previewed thing as a renderer lists it: a slide; later a sheet, a
/// clip's in-point or waveform, a camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    /// Its address on the wire (`@sld:256`).
    pub key: String,
    /// From 1, in the doc's order.
    pub position: u32,
    pub title: String,
    pub hidden: bool,
    /// The raw digest of everything its drawing reads (the pack's render
    /// closure). Never leaves the host as is: [`service`] keys it.
    pub digest: [u8; 32],
    /// What the renderer needs to find it again (a deck: the slide's
    /// `sldId`; a timeline: a media key and a source time). Opaque to the
    /// host, never on the wire.
    pub locator: String,
    /// What the type shows beside it, as the listing sends it (a scene
    /// object's local box, a camera's display values; empty for a slide).
    /// Never a digest.
    pub extras: serde_json::Map<String, serde_json::Value>,
}

/// A file's containers, as a renderer lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The page every container is drawn on; `None` for a type without one.
    pub page: Option<Page>,
    pub containers: Vec<Container>,
}

/// A drawn container.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub bytes: Vec<u8>,
    /// `image/webp` or `image/png`.
    pub mime: &'static str,
    /// The contract's `format` word (`webp-lossless`, `png`).
    pub format: &'static str,
    pub width: u32,
    pub height: u32,
    /// Families the file asked for that were drawn in another: `(asked, drawn)`.
    pub fonts_substituted: Vec<(String, String)>,
    /// A scene drawn from base meshes, without some object's modifiers (the
    /// view says "shown without modifiers"). Always false for a deck.
    pub without_modifiers: bool,
    pub render_ms: u32,
    /// What the type reports with the image, kept in its metadata and sent
    /// with `preview_ready` (a scene view's camera and id table; empty for a
    /// slide).
    pub extras: serde_json::Map<String, serde_json::Value>,
    /// A scene view's id image (PNG: which object is under each pixel; its
    /// table is `extras["ids"]`), cached beside the image and served by the
    /// route's `layer=ids`. `None` for a slide and for a view drawn without
    /// one.
    pub ids: Option<Vec<u8>>,
}

/// Files a renderer may read besides the doc itself, resolved by the host
/// inside the doc's workspace (a timeline's media). Host-only: a key and a
/// path never reach the wire or the cache's names.
pub trait SideFiles {
    fn resolve(&self, media_key: &str) -> Option<std::path::PathBuf>;
}

/// No side files (a deck draws from its own bytes).
pub struct NoSideFiles;

impl SideFiles for NoSideFiles {
    fn resolve(&self, _media_key: &str) -> Option<std::path::PathBuf> {
        None
    }
}

/// What a render draws from: the doc's bytes at the rev or version asked
/// (`file_now` / `file_at`), and its side files.
pub struct Source<'a> {
    pub doc: &'a [u8],
    pub side: &'a dyn SideFiles,
}

/// One doc type's renderer.
pub trait PreviewRenderer: Send + Sync {
    /// Its short id (`deck`): in the cache's file names and the route's answer.
    fn kind(&self) -> &'static str;
    /// Its build (protocol and version): part of the cache key, so a new
    /// renderer never serves an old one's images.
    fn build(&self) -> String;
    /// Whether it draws this doc as it is served now (its core on, its tier).
    fn serves(&self, t: &Target) -> bool;
    /// The look a request gets when it names none.
    fn default_look(&self) -> Look {
        Look::Opaque
    }
    /// Whether it can draw this look.
    fn draws(&self, look: Look) -> bool {
        look == Look::Opaque
    }
    /// Whether `container`'s images come with an id image ([`Image::ids`]).
    fn draws_ids(&self, _container: &Container) -> bool {
        false
    }
    /// The containers of `bytes`, with their raw digests. Runs in the
    /// daemon on bytes the live pack already holds (bounded by the pack's
    /// limits); never draws.
    fn list(&self, bytes: &[u8]) -> Result<Listing, Failed>;
    /// Draw `container` of `src` at `width` px in `look` (one it
    /// [`draws`](PreviewRenderer::draws)), out of process, within `wall`.
    /// Blocking.
    fn render(&self, src: &Source<'_>, container: &Container, width: Width, look: Look, wall: std::time::Duration) -> Result<Image, Failed>;
}

/// Every renderer, in the order they are asked.
pub fn renderers() -> &'static [&'static dyn PreviewRenderer] {
    static ALL: [&dyn PreviewRenderer; 2] = [&deck::DECK, &scene::SCENE];
    &ALL
}

/// The renderer for a doc as it is served now, if any type draws it.
pub fn renderer_for(t: &Target) -> Option<&'static dyn PreviewRenderer> {
    renderers().iter().copied().find(|r| r.serves(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_are_the_contracts_buckets() {
        assert_eq!(Width::ALL.map(Width::px), [320, 640, 1920]);
        for w in Width::ALL {
            assert_eq!(Width::parse(w.as_str()), Some(w));
        }
        assert_eq!(Width::parse("huge"), None);
        assert!(Width::Full.max_bytes() <= super::super::live_wire::FRAME_MAX as u64, "a full image fits the docs socket's frame");
    }

    #[test]
    fn failure_codes_are_the_contracts_four_and_the_hosts_own() {
        let words: Vec<&str> = [FailCode::TooLarge, FailCode::UnsupportedPart, FailCode::Timeout, FailCode::RendererError, FailCode::NotOnThisHost]
            .map(FailCode::as_str)
            .to_vec();
        assert_eq!(words, ["too_large", "unsupported_part", "timeout", "renderer_error", "not_on_this_host"]);
    }

    #[test]
    fn only_a_live_deck_has_a_renderer_today() {
        let t = |rel: &str, tier| Target {
            project_id: "p".into(),
            root: std::path::PathBuf::from("/w"),
            rel: rel.into(),
            abs: std::path::PathBuf::from("/w").join(rel),
            tier,
        };
        use super::super::target::Tier;
        assert_eq!(renderer_for(&t("talk.pptx", Tier::Deck)).map(|r| r.kind()), Some("deck"));
        assert!(renderer_for(&t("talk.pptx", Tier::Blob)).is_none(), "a deck served at tier 0 (Slides off)");
        assert!(renderer_for(&t("notes.md", Tier::Markdown)).is_none());
        assert!(renderer_for(&t("photo.png", Tier::Blob)).is_none());
        // The scene renderer is registered, but serves nothing until the BLEND pack's mesh read lands.
        assert!(renderers().iter().any(|r| r.kind() == "scene"));
        assert!(renderer_for(&t("scene.blend", Tier::Blob)).is_none());
    }
}
