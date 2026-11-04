//! Limits, decided before parsing. The format sets the ceilings; a host may only tighten them.

use crate::refusal::{Refusal, RefusalCode};

const MIB: u64 = 1024 * 1024;

/// Resource limits for one reader or writer. Start from [`Limits::V1`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Zip entries, the manifest included. 65,535 is the ZIP64 marker, so 65,534 is the most.
    pub entries: u64,
    /// `manifest.json`, uncompressed.
    pub manifest_bytes: u64,
    /// `board.json`, uncompressed.
    pub board_json_bytes: u64,
    /// Any other part, uncompressed.
    pub part_bytes: u64,
    /// All entries together, uncompressed.
    pub total_bytes: u64,
    /// Output bytes per compressed byte, held while inflating.
    pub ratio: u64,
    /// Output below this many bytes is exempt from `ratio`.
    pub ratio_floor: u64,
    /// Objects in a board.
    pub objects: u64,
    /// Entries in a board's `people`.
    pub people: u64,
    /// JSON nesting.
    pub depth: u64,
    /// JSON values in one document, which bounds the memory a parse can take.
    pub values: u64,
}

impl Limits {
    /// The format's ceilings for version 1.
    pub const V1: Limits = Limits {
        entries: 65_534,
        manifest_bytes: 32 * MIB,
        board_json_bytes: 256 * MIB,
        part_bytes: 128 * MIB,
        total_bytes: 2048 * MIB,
        ratio: 300,
        ratio_floor: MIB,
        objects: 100_000,
        people: 100_000,
        depth: 64,
        values: 20_000_000,
    };

    /// Each limit by name, as in the schema and the spec.
    pub fn named(&self) -> [(&'static str, u64); 11] {
        [
            ("entries", self.entries),
            ("manifest_bytes", self.manifest_bytes),
            ("board_json_bytes", self.board_json_bytes),
            ("part_bytes", self.part_bytes),
            ("total_bytes", self.total_bytes),
            ("ratio", self.ratio),
            ("ratio_floor", self.ratio_floor),
            ("objects", self.objects),
            ("people", self.people),
            ("depth", self.depth),
            ("values", self.values),
        ]
    }

    /// Refused with `limit_raised` if any limit is above the format's ceiling. Every entry point
    /// runs this, so a host can tighten a limit but never raise one.
    pub fn check_ceiling(&self) -> Result<(), Refusal> {
        for ((name, ours), (_, ceiling)) in self.named().into_iter().zip(Limits::V1.named()) {
            if ours > ceiling {
                return Err(Refusal::at(
                    RefusalCode::LimitRaised,
                    name,
                    format!("{ours} is above the format's {ceiling}"),
                ));
            }
        }
        Ok(())
    }
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::V1
    }
}
