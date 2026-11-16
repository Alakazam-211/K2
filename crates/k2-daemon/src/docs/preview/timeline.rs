//! Live timelines: lane tiles, the third [`super::PreviewRenderer`] (a slot
//! today: no timeline is served yet).
//!
//! - **What it previews** (Design's live timeline notes v2.7b, "Lane
//!   tiles"): one **head tile** per clip, decoded at its in-point, and one
//!   **tail tile**, at its out-point. Each is keyed by (media hash, source
//!   time, width): the digest is the clip's media content hash and the
//!   source time in the timeline's unit ([`tile_digest`]), never the clip's
//!   place on the timeline or its name, and the host keys the width. So a
//!   move needs no new picture, a trim of the start one new head tile, a
//!   trim of the end one new tail tile, and two clips cut from the same
//!   source at the same in-point share one image (the host's [`super::reuse`]
//!   answers the second from the first). The filmstrip, posters and
//!   waveforms are K2 Render's derivatives service, later.
//! - **Addresses:** a tile is its clip's address with a `tile:head` or
//!   `tile:tail` part (`@trk:1/clip:7/tile:head`), in the kit's grammar.
//!   Its locator (host-only) names the media and the time.
//! - **Not on this host.** Tiles are decoded with the host's own video
//!   decoders (K2 Render's tile worker, OS decoders), so a host that has
//!   none answers [`FailCode::NotOnThisHost`] for every tile
//!   ([`PreviewRenderer::decodes_here`] false): the host answers at once,
//!   queues nothing, keeps no failure (the doc changing doesn't change the
//!   host) and the view shows its "no preview" card. **No host has a tile
//!   decoder yet** ([`decoder_here`]), so today every tile is
//!   `not_on_this_host`.
//! - **Not served yet** ([`TimelineRenderer::serves`]): the timeline pack
//!   isn't vendored and a timeline file has no tier or core, so [`list`] has
//!   no structure to read. Once it has, the pack's view index gives the
//!   clips ([`Tile`]) and [`listing`] keys them.
//!
//! [`list`]: TimelineRenderer::list

use sha2::{Digest as _, Sha256};

use super::{Container, FailCode, Failed, Image, Listing, Look, PreviewRenderer, Shape, Source, Width};
use crate::docs::target::Target as DocTarget;

/// The digest's domain label: a tile's digest is never another type's.
const DIGEST_LABEL: &[u8] = b"k2/timeline-tile/v1";

pub struct TimelineRenderer;

pub static TIMELINE: TimelineRenderer = TimelineRenderer;

/// Which end of a clip a tile shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// At the clip's in-point.
    Head,
    /// At the clip's out-point.
    Tail,
}

impl Edge {
    pub fn as_str(self) -> &'static str {
        match self {
            Edge::Head => "head",
            Edge::Tail => "tail",
        }
    }
}

/// One clip as the timeline pack's view index gives it: what its tiles are
/// drawn from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    /// The clip's address (`@trk:1/clip:7`).
    pub clip: String,
    /// The clip's name (the tile's title).
    pub name: String,
    /// The media's content hash (32 bytes), as the pack reads its source.
    pub media_hash: [u8; 32],
    /// The media's key for the host's side files ([`super::SideFiles`]):
    /// host-only, in the locator, never on the wire.
    pub media_key: String,
    /// The in-point and out-point in the source's time, in the timeline's
    /// unit (ticks), as the pack gives them.
    pub source_in: i64,
    pub source_out: i64,
}

/// A tile's raw digest: its media's content hash and the source time, only.
pub fn tile_digest(media_hash: &[u8; 32], source_time: i64) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(DIGEST_LABEL);
    h.update(media_hash);
    h.update(source_time.to_string().as_bytes());
    h.finalize().into()
}

/// A tile's address: its clip's, then `tile:head` or `tile:tail`.
pub fn tile_address(clip: &str, edge: Edge) -> String {
    format!("{clip}/tile:{}", edge.as_str())
}

/// The clips' tiles, head then tail, in the clips' order: no page.
pub fn listing(clips: &[Tile]) -> Listing {
    let mut containers = Vec::with_capacity(clips.len() * 2);
    for c in clips {
        for (edge, at) in [(Edge::Head, c.source_in), (Edge::Tail, c.source_out)] {
            containers.push(Container {
                key: tile_address(&c.clip, edge),
                position: containers.len() as u32 + 1,
                title: c.name.clone(),
                hidden: false,
                digest: tile_digest(&c.media_hash, at),
                locator: format!("{}@{at}", c.media_key),
                extras: serde_json::Map::new(),
                live_key: None,
            });
        }
    }
    Listing { page: None, containers, extras: serde_json::Map::new() }
}

/// Whether this host can decode video for tiles.
///
/// TODO(K2 Render): true where its tile worker and the OS decoders it needs
/// are here (the worker's own probe, as `k2-scenerender` sits beside the
/// daemon). No such worker exists yet, so no host can.
pub fn decoder_here() -> bool {
    false
}

impl PreviewRenderer for TimelineRenderer {
    fn kind(&self) -> &'static str {
        "timeline"
    }

    /// No worker yet: the slot's own build, so no image a later worker
    /// draws is ever confused with one from before it.
    fn build(&self) -> String {
        "timeline-tiles-0".into()
    }

    /// No doc yet. TODO(K2): vendor the timeline pack (after K2 Cores tags
    /// it and the Steward clears its licence), give timeline files their
    /// tier and the "Timelines" core, and serve them here.
    fn serves(&self, _t: &DocTarget) -> bool {
        false
    }

    fn decodes_here(&self, _container: &Container) -> bool {
        decoder_here()
    }

    /// The pack's view index gives the clips ([`listing`]); it isn't in this
    /// build.
    fn list(&self, _bytes: &[u8]) -> Result<Listing, Failed> {
        Err(Failed::new(FailCode::UnsupportedPart, "the timeline pack isn't in this build"))
    }

    /// Never reached while [`decoder_here`] is false (the host answers
    /// first); the same answer if it were.
    fn render(&self, _src: &Source<'_>, container: &Container, _width: Width, _look: Look, _shape: Shape, _wall: std::time::Duration) -> Result<Image, Failed> {
        Err(Failed::new(FailCode::NotOnThisHost, format!("no video decoder for {} on this host", container.key)))
    }
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod tests;
