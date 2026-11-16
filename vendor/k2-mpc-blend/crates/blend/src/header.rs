//! The file header: which of the two header forms a file has, its pointer size, its byte order
//! and the version of the program that saved it. `PROVENANCE.md` says where each layout comes
//! from.
//!
//! **The classic form** (12 bytes): `BLENDER`, a pointer size (`_` 4 bytes, `-` 8 bytes), a byte
//! order (`v` little-endian, `V` big-endian), and a three-digit version (`403` is 4.03).
//!
//! **The newer form** (larger blocks): `BLENDER`, the header's own length as two digits (`17`),
//! the pointer size (`-`), a two-digit block-header format (`01`), the byte order (`v`), and a
//! four-digit version (`0501` is 5.01). Every value this reader hasn't seen in sample files is
//! refused as [`ReadError::Unknown`]: another header length, pointer size, format or byte order.

use crate::error::ReadError;

/// The magic every uncompressed file starts with.
pub const MAGIC: &[u8; 7] = b"BLENDER";
/// The longest header: what a reader reads before it knows the form.
pub const HEADER_MAX: usize = 17;

/// Which form a file's header (and so its block headers) takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HeaderForm {
    /// 12-byte header; block headers of 16 bytes plus a pointer, with a 4-byte body length.
    Classic,
    /// The newer header; block headers of 32 bytes, with 8-byte lengths and counts.
    Large,
}

/// A file header, read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Header {
    /// Which form.
    pub form: HeaderForm,
    /// The size of a pointer in the file: 4 or 8.
    pub pointer_size: u8,
    /// Numbers in the file are big-endian.
    pub big_endian: bool,
    /// The saving program's version as the file writes it: `403` for 4.03, `501` for 5.01.
    pub version: u16,
    /// The header's length in bytes: where the first block starts.
    pub len: u8,
}

impl Header {
    /// A block header's length in bytes.
    pub fn block_header_len(&self) -> usize {
        match self.form {
            HeaderForm::Classic => 16 + usize::from(self.pointer_size),
            HeaderForm::Large => 32,
        }
    }

    /// `major.minor`, as `4.03`.
    pub fn version_text(&self) -> String {
        format!("{}.{:02}", self.version / 100, self.version % 100)
    }
}

fn digits(b: &[u8]) -> Option<u16> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    b.iter().try_fold(0u16, |v, d| {
        v.checked_mul(10)?.checked_add(u16::from(d - b'0'))
    })
}

/// Reads a header from the start of `bytes` (at least [`HEADER_MAX`] bytes when the file has
/// them). [`ReadError::NotBlend`] when the magic is missing.
pub fn parse_header(bytes: &[u8]) -> Result<Header, ReadError> {
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        return Err(ReadError::NotBlend);
    }
    let need = |n: usize| {
        if bytes.len() < n {
            Err(ReadError::truncated("the file header"))
        } else {
            Ok(())
        }
    };
    need(8)?;
    match bytes[7] {
        b'_' | b'-' => {
            need(12)?;
            let pointer_size = if bytes[7] == b'-' { 8 } else { 4 };
            let big_endian = match bytes[8] {
                b'v' => false,
                b'V' => true,
                _ => return Err(ReadError::Unknown { what: "byte order" }),
            };
            let version = digits(&bytes[9..12]).ok_or(ReadError::Unknown { what: "version" })?;
            Ok(Header {
                form: HeaderForm::Classic,
                pointer_size,
                big_endian,
                version,
                len: 12,
            })
        }
        b'0'..=b'9' => {
            need(9)?;
            let len = digits(&bytes[7..9]).ok_or(ReadError::Unknown {
                what: "header length",
            })?;
            if len != 17 {
                return Err(ReadError::Unknown {
                    what: "header length",
                });
            }
            need(17)?;
            if bytes[9] != b'-' {
                return Err(ReadError::Unknown {
                    what: "pointer size",
                });
            }
            if &bytes[10..12] != b"01" {
                return Err(ReadError::Unknown {
                    what: "block header format",
                });
            }
            if bytes[12] != b'v' {
                return Err(ReadError::Unknown { what: "byte order" });
            }
            let version = digits(&bytes[13..17]).ok_or(ReadError::Unknown { what: "version" })?;
            Ok(Header {
                form: HeaderForm::Large,
                pointer_size: 8,
                big_endian: false,
                version,
                len: 17,
            })
        }
        _ => Err(ReadError::Unknown {
            what: "pointer size",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_forms_read() {
        let h = parse_header(b"BLENDER-v403REND").unwrap();
        assert_eq!(h.form, HeaderForm::Classic);
        assert_eq!(
            (h.pointer_size, h.big_endian, h.version, h.len),
            (8, false, 403, 12)
        );
        assert_eq!(h.block_header_len(), 24);
        assert_eq!(h.version_text(), "4.03");
        let h = parse_header(b"BLENDER_V279").unwrap();
        assert_eq!((h.pointer_size, h.big_endian, h.version), (4, true, 279));
        assert_eq!(h.block_header_len(), 20);
        let h = parse_header(b"BLENDER17-01v0501REND").unwrap();
        assert_eq!(h.form, HeaderForm::Large);
        assert_eq!((h.pointer_size, h.version, h.len), (8, 501, 17));
        assert_eq!(h.block_header_len(), 32);
        assert_eq!(h.version_text(), "5.01");
    }

    #[test]
    fn anything_unseen_is_refused_by_name() {
        let unknown = |b: &[u8]| match parse_header(b) {
            Err(ReadError::Unknown { what }) => what,
            other => panic!("{other:?}"),
        };
        assert_eq!(unknown(b"BLENDER18-01v0501x"), "header length");
        assert_eq!(unknown(b"BLENDER17_01v0501"), "pointer size");
        assert_eq!(unknown(b"BLENDER17-02v0501"), "block header format");
        assert_eq!(unknown(b"BLENDER17-01V0501"), "byte order");
        assert_eq!(unknown(b"BLENDER17-01v05x1"), "version");
        assert_eq!(unknown(b"BLENDER-x403"), "byte order");
        assert_eq!(unknown(b"BLENDER*v403"), "pointer size");
        assert!(matches!(
            parse_header(b"BLENDEX-v403"),
            Err(ReadError::NotBlend)
        ));
        assert!(matches!(
            parse_header(b"BLENDER17-01v05"),
            Err(ReadError::Truncated { .. })
        ));
    }
}
