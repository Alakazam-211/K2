//! Content-defined chunking of a version's bytes (`docs/design/version-storage-r2.md`,
//! section 1): a gear rolling hash with a minimum, an average and a maximum chunk size, and
//! normalized cuts (a harder mask before the average, an easier one after it). Boundaries
//! depend only on the bytes: the same bytes always cut the same way, and an edit moves the cuts
//! near it only.
//!
//! **The cut is part of the stored format.** The gear table (a fixed seed), the masks and the
//! sizes decide which chunks a version shares with another; changing any of them changes no
//! stored byte, but stops new versions sharing chunks with old ones. A doc's sizes are fixed at
//! its first chunked version and kept in each manifest ([`Sizes::code`]).

/// A chunker's minimum, average and maximum chunk sizes, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sizes {
    pub(crate) min: usize,
    pub(crate) avg: usize,
    pub(crate) max: usize,
}

/// The sizes for a file under [`LARGE_FROM`]: 16 KiB, 64 KiB, 256 KiB.
pub(crate) const SMALL: Sizes = Sizes {
    min: 16 << 10,
    avg: 64 << 10,
    max: 256 << 10,
};

/// The sizes for a file of [`LARGE_FROM`] and more: 64 KiB, 256 KiB, 1 MiB.
pub(crate) const LARGE: Sizes = Sizes {
    min: 64 << 10,
    avg: 256 << 10,
    max: 1 << 20,
};

/// The file size from which a doc's first chunked version takes [`LARGE`]: 64 MiB.
pub(crate) const LARGE_FROM: u64 = 64 << 20;

impl Sizes {
    /// The sizes a doc takes at its first chunked version, from that version's length.
    pub(crate) fn for_first(len: u64) -> Sizes {
        if len >= LARGE_FROM {
            LARGE
        } else {
            SMALL
        }
    }

    /// The code a manifest stores: 0 for [`SMALL`], 1 for [`LARGE`].
    pub(crate) fn code(self) -> u8 {
        u8::from(self == LARGE)
    }

    /// The sizes a manifest's code names (`None`: a code this build doesn't know).
    pub(crate) fn from_code(c: u8) -> Option<Sizes> {
        match c {
            0 => Some(SMALL),
            1 => Some(LARGE),
            _ => None,
        }
    }
}

/// The gear table's seed (splitmix64): fixed, part of the cut.
const GEAR_SEED: u64 = 0x6b32_6d70_6363_6463;

/// The chunker for one set of sizes.
pub(crate) struct Cdc {
    sizes: Sizes,
    gear: [u64; 256],
    mask_s: u64,
    mask_l: u64,
}

impl Cdc {
    pub(crate) fn new(sizes: Sizes) -> Cdc {
        let mut s = GEAR_SEED;
        let mut gear = [0u64; 256];
        for g in gear.iter_mut() {
            s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            *g = z ^ (z >> 31);
        }
        let bits = sizes.avg.trailing_zeros().max(2);
        let mask = |b: u32| ((1u64 << b) - 1) << (64 - b);
        Cdc {
            sizes,
            gear,
            mask_s: mask(bits + 2),
            mask_l: mask(bits - 2),
        }
    }

    /// The largest chunk this chunker cuts.
    pub(crate) fn max(&self) -> usize {
        self.sizes.max
    }

    /// The smallest chunk this chunker cuts (but at the end of its input).
    pub(crate) fn min(&self) -> usize {
        self.sizes.min
    }

    /// The length of the chunk at the start of `b` (all of `b` when it is at most the minimum).
    pub(crate) fn cut(&self, b: &[u8]) -> usize {
        let Sizes { min, avg, max } = self.sizes;
        if b.len() <= min {
            return b.len();
        }
        let end = b.len().min(max);
        let mid = end.min(avg);
        let mut h = 0u64;
        let mut i = min;
        while i < mid {
            h = (h << 1).wrapping_add(self.gear[usize::from(b[i])]);
            if h & self.mask_s == 0 {
                return i + 1;
            }
            i += 1;
        }
        while i < end {
            h = (h << 1).wrapping_add(self.gear[usize::from(b[i])]);
            if h & self.mask_l == 0 {
                return i + 1;
            }
            i += 1;
        }
        end
    }

    /// `b`'s chunks, as (offset, length), in order, covering it exactly.
    pub(crate) fn chunks(&self, b: &[u8]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < b.len() {
            let n = self.cut(&b[at..]);
            out.push((at, n));
            at += n;
        }
        out
    }

    /// `b`'s chunks with `cuts` (checked hints, [`check_hints`]) as allowed cut points: the
    /// chunker's own cut, restarted at each one, so a span between two is cut by content and at
    /// the maximum as any input is. `content` false is the planted fault `hints-required`: each
    /// span is one chunk, however long.
    pub(crate) fn chunks_at(&self, b: &[u8], cuts: &[usize], content: bool) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut start = 0;
        for end in cuts.iter().copied().chain(std::iter::once(b.len())) {
            if content {
                out.extend(
                    self.chunks(&b[start..end])
                        .into_iter()
                        .map(|(o, n)| (start + o, n)),
                );
            } else if end > start {
                out.push((start, end - start));
            }
            start = end;
        }
        out
    }
}

/// A pack's cut hints for `len` bytes, as cut points (`docs/design/version-storage-r2.md`,
/// section 13): `None`, all of them ignored, when one is outside `(0, len)` or they aren't in
/// increasing order (a repeat included). A hint closer than `min` to the last cut point kept
/// (or to the start) is merged into it, the first of a run kept; at most `len / min` are kept.
/// `merge` false is the planted fault `hints-unmerged`.
pub(crate) fn check_hints(raw: &[u64], len: usize, min: usize, merge: bool) -> Option<Vec<usize>> {
    let mut prev = 0u64;
    for &h in raw {
        if h <= prev || h >= len as u64 {
            return None;
        }
        prev = h;
    }
    let mut kept = Vec::new();
    let mut last = 0usize;
    for &h in raw {
        let h = usize::try_from(h).ok()?;
        if !merge || h - last >= min {
            kept.push(h);
            last = h;
        }
    }
    kept.truncate(len / min.max(1));
    Some(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic bytes (splitmix64 from `seed`).
    fn bytes(seed: u64, n: usize) -> Vec<u8> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
                let mut z = s;
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                (z >> 56) as u8
            })
            .collect()
    }

    /// The gear table is pinned: its first and last entries. A change here changes which chunks
    /// new versions share with stored ones.
    #[test]
    fn the_gear_table_is_pinned() {
        let c = Cdc::new(SMALL);
        assert_eq!(
            c.gear[0], 0x0c60_2f78_9a9c_082e,
            "gear[0]: {:#x}",
            c.gear[0]
        );
        assert_eq!(
            c.gear[255], 0xeeb6_80e4_895f_fc0e,
            "gear[255]: {:#x}",
            c.gear[255]
        );
    }

    /// Chunks cover the bytes exactly, each within the sizes (the last may be short), and the
    /// same bytes always cut the same way.
    #[test]
    fn chunks_cover_the_bytes_within_the_sizes() {
        for sizes in [SMALL, LARGE] {
            let c = Cdc::new(sizes);
            let b = bytes(7, 3 * sizes.max + 12_345);
            let ch = c.chunks(&b);
            let mut at = 0;
            for (i, &(o, n)) in ch.iter().enumerate() {
                assert_eq!(o, at);
                assert!(n <= sizes.max, "chunk {i} is {n}");
                if i + 1 < ch.len() {
                    assert!(n > sizes.min, "chunk {i} is {n}");
                }
                at += n;
            }
            assert_eq!(at, b.len());
            assert_eq!(c.chunks(&b), ch, "deterministic");
        }
        assert!(Cdc::new(SMALL).chunks(&[]).is_empty());
        assert_eq!(Cdc::new(SMALL).chunks(&[1, 2, 3]), vec![(0, 3)]);
    }

    /// An edit in the middle of a file moves only the cuts near it: most chunks after it are the
    /// same bytes as before.
    #[test]
    fn an_edit_keeps_the_chunks_away_from_it() {
        let c = Cdc::new(SMALL);
        let a = bytes(11, 4 << 20);
        let mut b = a.clone();
        b.splice(2 << 20..(2 << 20) + 10, [0u8; 37]);
        let set = |x: &[u8]| -> std::collections::HashSet<Vec<u8>> {
            c.chunks(x)
                .iter()
                .map(|&(o, n)| x[o..o + n].to_vec())
                .collect()
        };
        let (sa, sb) = (set(&a), set(&b));
        let shared = sb.iter().filter(|k| sa.contains(*k)).count();
        assert!(
            shared + 4 >= sb.len(),
            "{shared} of {} chunks shared",
            sb.len()
        );
    }

    /// Hints are checked (out of range or out of order: all ignored), merged up to the minimum
    /// (the first of a run kept) and capped; between them the cut is the chunker's own, so no
    /// chunk is over the maximum, and no hints cut as [`Cdc::chunks`] does.
    #[test]
    fn hints_are_checked_merged_and_cut_between() {
        let min = SMALL.min;
        assert_eq!(check_hints(&[], 100, min, true), Some(vec![]));
        for bad in [&[0u64][..], &[100], &[101], &[50, 40], &[40, 40]] {
            assert_eq!(check_hints(bad, 100, 10, true), None, "{bad:?}");
        }
        assert_eq!(
            check_hints(&[4, 10, 14, 20, 35], 100, 10, true),
            Some(vec![10, 20, 35])
        );
        assert_eq!(
            check_hints(&[4, 10, 14], 100, 10, false),
            Some(vec![4, 10, 14])
        );
        let dense: Vec<u64> = (1..1000).collect();
        assert!(check_hints(&dense, 1000, 1, false).unwrap().len() <= 1000);
        assert_eq!(check_hints(&dense, 1000, 100, true).unwrap().len(), 9);
        let c = Cdc::new(SMALL);
        let b = bytes(5, 3 * SMALL.max + 777);
        assert_eq!(
            c.chunks_at(&b, &[], true),
            c.chunks(&b),
            "no hints: the plain cut"
        );
        let cuts =
            check_hints(&[min as u64 * 3, SMALL.max as u64 * 2], b.len(), min, true).unwrap();
        let ch = c.chunks_at(&b, &cuts, true);
        let mut at = 0;
        for &(o, n) in &ch {
            assert_eq!(o, at);
            assert!(n <= SMALL.max, "a chunk of {n}");
            at += n;
        }
        assert_eq!(at, b.len());
        for cut in &cuts {
            assert!(ch.iter().any(|&(o, _)| o == *cut), "a cut at {cut}");
        }
        assert_eq!(c.chunks_at(&b, &cuts, true), ch, "deterministic");
    }

    #[test]
    fn sizes_are_chosen_and_coded() {
        assert_eq!(Sizes::for_first(LARGE_FROM - 1), SMALL);
        assert_eq!(Sizes::for_first(LARGE_FROM), LARGE);
        for s in [SMALL, LARGE] {
            assert_eq!(Sizes::from_code(s.code()), Some(s));
        }
        assert_eq!(Sizes::from_code(2), None);
    }
}
