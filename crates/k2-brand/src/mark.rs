//! The brand mark: which brand a binary was built for, as bytes a release
//! reads without running it (`scripts/brand/check-brand-mark.py`).
//!
//! [`BRAND_MARK`] is `K5_BRAND_MARK=<brand id>`, then NULs to 32 bytes. A
//! program keeps it in its binary with [`keep`], one call at the top of
//! `main`. That is static data only: [`keep`] returns nothing, and a
//! helper program (ptyd, the node runtime, the App engine) never reads the
//! mark to decide anything (`scripts/brand/mark-static-gate.sh` holds them
//! to that). The daemon and the command line read it
//! ([`marked_brand_id`], through `k2_client::brand::session`) only to name
//! the brand in a header, an environment value or a warning line.

use crate::BRAND_ID;

/// What the mark's bytes start with; the id follows, up to the NUL.
pub const BRAND_MARK_PREFIX: &str = "K5_BRAND_MARK=";
const MARK_LEN: usize = 32;

/// The mark: [`BRAND_MARK_PREFIX`], this build's brand id, then NULs.
#[used]
pub static BRAND_MARK: [u8; MARK_LEN] = mark(BRAND_ID);

const fn mark(id: &str) -> [u8; MARK_LEN] {
    let prefix = BRAND_MARK_PREFIX.as_bytes();
    let id = id.as_bytes();
    assert!(
        prefix.len() + id.len() < MARK_LEN,
        "the brand id is too long for the brand mark"
    );
    let mut out = [0u8; MARK_LEN];
    let mut i = 0;
    while i < prefix.len() {
        out[i] = prefix[i];
        i += 1;
    }
    let mut j = 0;
    while j < id.len() {
        out[i + j] = id[j];
        j += 1;
    }
    out
}

/// Keeps the mark in this program's binary: the call takes its address,
/// so the linker can't drop it. Reads nothing, decides nothing.
#[inline(never)]
pub fn keep() {
    std::hint::black_box(&BRAND_MARK);
}

/// The brand id this binary carries, read from its mark.
pub fn marked_brand_id() -> &'static str {
    let mark: &'static [u8; MARK_LEN] = std::hint::black_box(&BRAND_MARK);
    let rest = &mark[BRAND_MARK_PREFIX.len()..];
    let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
    std::str::from_utf8(&rest[..end]).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_holds_this_build_s_brand_id() {
        keep();
        assert_eq!(marked_brand_id(), BRAND_ID);
        let mut want = format!("K5_BRAND_MARK={BRAND_ID}").into_bytes();
        want.resize(MARK_LEN, 0);
        assert_eq!(BRAND_MARK.to_vec(), want);
        assert_eq!(&mark("ab")[..17], b"K5_BRAND_MARK=ab\0");
    }
}
