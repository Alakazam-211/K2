//! The live pack's own refusals ([`k2_mpc_base::session::Pack::errors`]): each verb's "no",
//! by a stable id (`TIMELINE-OTIO-Ennn`, never reused) and the code its text prints
//! (`refused: <code>: <why>`, then `next: <call>`). The report a host gets carries the entry,
//! `why` as the message, and the details `code`, `next` (the call to run next, as the text's
//! `next:` line) and `verb`. Nothing applies on any of them.

use k2_mpc_base::errors::ErrorEntry;
use k2_mpc_base::tools::exit::{NOT_FOUND, USAGE};

/// Another item is where the clip would go.
pub const OVERLAP: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E001",
    "tl_overlap",
    USAGE,
    "another item is there",
    "Another item holds part of the range the clip would take. Nothing was applied. List the \
     track's items there and pick a free range, or move the other item first.",
)
.step(
    "clip list",
    &[],
    "See what is on the track around that time.",
);

/// A time between two frames, with no `--snap`.
pub const OFF_GRID: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E002",
    "tl_off_grid",
    USAGE,
    "the time falls between two frames",
    "The time given is between two frames of the timeline's rate. Nothing was applied. Give a \
     time on a frame, or run the same call with --snap to take the nearer frame.",
)
.step(
    "timeline info",
    &[],
    "Read the timeline's rate, then give a time on one of its frames (or add --snap).",
);

/// A time it can't read.
pub const TIME: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E003",
    "tl_time",
    USAGE,
    "the time can't be read",
    "The time given isn't a timecode, frames (240f) or seconds (10s, 1001/24000s) at the \
     timeline's rate, or the timeline has no rate, or a relative time has nothing to count \
     from. Nothing was applied.",
)
.step(
    "timeline info",
    &[],
    "Read the timeline's rate and start timecode, then give the time again.",
);

/// A transition joins the clip to a neighbour, or sits on the edge it would trim.
pub const TRANSITION: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E004",
    "tl_transition",
    USAGE,
    "a transition joins the clip to its neighbour",
    "A transition sits on the clip's cut, and this edit would break it. Nothing was applied. \
     Edit the clip's other edge, or a clip with no transition beside it.",
)
.step("clip show", &[], "Read the clip and what sits beside it.");

/// `--track` names a track of another kind.
pub const TRACK_KIND: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E005",
    "tl_track_kind",
    USAGE,
    "the track carries another kind",
    "A clip goes only on a track of its own kind (video on video, audio on audio). Nothing was \
     applied. Pick a track of the clip's kind.",
)
.step("track list", &[], "List the tracks with their kinds.");

/// An item nothing writes live: a transition, a nest or a retimed clip.
pub const NOT_EDITABLE: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E006",
    "tl_not_editable",
    USAGE,
    "nothing writes this item live",
    "The item is a transition, a nest or a retimed clip, which the live timeline reads but \
     doesn't write. Nothing was applied. Edit it in the editing app.",
)
.step("clip show", &[], "Read the item's fields.");

/// A track whose gaps hold more than a duration.
pub const TRACK_NOT_EDITABLE: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E007",
    "tl_track_not_editable",
    USAGE,
    "a gap on this track holds more than a duration",
    "A gap on the track carries a name, markers, effects or metadata, which the write-back \
     can't rebuild, so nothing is placed on the track live. Nothing was applied. Use another \
     track, or edit this one in the editing app.",
)
.step("track list", &[], "List the tracks.");

/// The media doesn't hold the frames asked for.
pub const MEDIA_RANGE: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E008",
    "tl_media_range",
    USAGE,
    "the media doesn't hold those frames",
    "The clip would show frames before its media's start or past its end. Nothing was applied. \
     Read the media's length and start, and keep the clip within it.",
)
.step("media list", &[], "Read each media's length and start.");

/// Out of range: an empty clip, a marker outside its owner, a negative duration, a time past
/// what a live timeline holds.
pub const RANGE: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E009",
    "tl_range",
    USAGE,
    "out of range",
    "The edit would make an empty clip, start before zero, end past the longest timeline held \
     live, put a marker outside its owner, or give a negative duration. Nothing was applied.",
)
.step(
    "timeline info",
    &[],
    "Read the timeline's length, then give times within it.",
);

/// A time or gap the clip's own rate, or its neighbours', can't write exactly.
pub const RATE: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E010",
    "tl_rate",
    USAGE,
    "that isn't a whole frame of the rate it is written at",
    "Each clip's times are written at its own rate, and the gaps around it at its neighbours'. \
     This edit leaves a time or a gap that one of those rates can't write exactly. Nothing was \
     applied. Give times on the clip's own frames.",
)
.step("clip show", &[], "Read the clip's rate and times.");

/// A track, item, marker or media that isn't there.
pub const NOT_FOUND_ENTRY: ErrorEntry = ErrorEntry::pack(
    "TIMELINE-OTIO-E011",
    "tl_not_found",
    NOT_FOUND,
    "the address names nothing live",
    "The address names no track, item, marker or media in the timeline as it is now: it was \
     deleted, or never was. Nothing was applied. List them again and use an address from the \
     list.",
)
.step(
    "track list",
    &[],
    "List the tracks, then their items, markers or media.",
);

/// The pack's own refusals, in id order.
pub const ERRORS: &[ErrorEntry] = &[
    OVERLAP,
    OFF_GRID,
    TIME,
    TRANSITION,
    TRACK_KIND,
    NOT_EDITABLE,
    TRACK_NOT_EDITABLE,
    MEDIA_RANGE,
    RANGE,
    RATE,
    NOT_FOUND_ENTRY,
];

/// The entry whose code is `code`, among [`ERRORS`].
pub fn by_code(code: &str) -> Option<&'static ErrorEntry> {
    ERRORS.iter().find(|e| e.name == code)
}
