//! Hand-written bytes for the rows: bit fields packed most significant first, and literal runs
//! encoded for the compressed streams.
#![allow(dead_code)]

use k2_mpc_dwg::CadError;

pub mod container;
pub mod object;

/// The fields `(value, width)` packed most significant bit first, the last byte padded with
/// zero bits.
pub fn pack(fields: &[(u64, u32)]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut n = 0usize;
    for &(v, w) in fields {
        for k in (0..w).rev() {
            if n % 8 == 0 {
                out.push(0);
            }
            if (v >> k) & 1 == 1 {
                let last = out.len() - 1;
                out[last] |= 0x80 >> (n % 8);
            }
            n += 1;
        }
    }
    out
}

/// `b` as eight-bit fields.
pub fn bytes(b: &[u8]) -> Vec<(u64, u32)> {
    b.iter().map(|&x| (u64::from(x), 8)).collect()
}

/// A two-bit code, then `b` as eight-bit fields.
pub fn coded(code: u64, b: &[u8]) -> Vec<(u64, u32)> {
    let mut f = vec![(code, 2)];
    f.extend(bytes(b));
    f
}

/// Asserts `got` is `Ok(want)`, naming `what`.
pub fn ok<T: std::fmt::Debug + PartialEq>(got: Result<T, CadError>, want: T, what: &str) {
    assert_eq!(got, Ok(want), "{what}");
}

/// Asserts `got` is refused with the code `id`, naming `what`.
pub fn refused<T: std::fmt::Debug>(got: Result<T, CadError>, id: &str, what: &str) {
    match got {
        Err(e) => assert_eq!(e.code.id, id, "{what}: {}", e.summary()),
        Ok(v) => panic!("{what}: read {v:?}, not refused with {id}"),
    }
}

/// A literal run's opcode and length bytes for `n` literals (`n` at least 4 at a stream's
/// start), then the literals.
pub fn literal(lits: &[u8]) -> Vec<u8> {
    let n = lits.len();
    assert!(n >= 4, "a literal run carries at least 4 bytes");
    let mut out = Vec::new();
    if n - 3 <= 0x0F {
        out.push((n - 3) as u8);
    } else {
        out.push(0x00);
        let mut rest = n - 3 - 0x0F;
        while rest > 0xFF {
            out.push(0x00);
            rest -= 0xFF;
        }
        // a length byte can't be zero: a run that ends on a multiple of 0xFF borrows one
        assert!(rest > 0, "pick another length");
        out.push(rest as u8);
    }
    out.extend_from_slice(lits);
    out
}
