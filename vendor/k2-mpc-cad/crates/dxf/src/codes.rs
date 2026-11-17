//! The type of a group code's value, by the code's range: our own table, written from the DXF
//! reference read as a specification.

/// What a value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    /// Text.
    Str,
    /// A real number.
    Double,
    /// A 16-bit integer.
    I16,
    /// A 32-bit integer.
    I32,
    /// A 64-bit integer.
    I64,
    /// 0 or 1.
    Bool,
    /// An object's own handle, or a pointer to one.
    Handle,
    /// Binary data as hexadecimal text.
    Hex,
    /// A comment (`999`).
    Comment,
    /// A code no range names.
    Unknown,
}

/// The type of code `c`'s value.
pub fn ty(c: i32) -> Ty {
    match c {
        5 | 105 => Ty::Handle,
        0..=9 => Ty::Str,
        10..=59 => Ty::Double,
        60..=79 => Ty::I16,
        90..=99 => Ty::I32,
        100 | 102 => Ty::Str,
        110..=149 => Ty::Double,
        160..=169 => Ty::I64,
        170..=179 => Ty::I16,
        210..=239 => Ty::Double,
        270..=289 => Ty::I16,
        290..=299 => Ty::Bool,
        300..=309 => Ty::Str,
        310..=319 => Ty::Hex,
        320..=369 => Ty::Handle,
        370..=389 => Ty::I16,
        390..=399 => Ty::Handle,
        400..=409 => Ty::I16,
        410..=419 => Ty::Str,
        420..=429 => Ty::I32,
        430..=439 => Ty::Str,
        440..=459 => Ty::I32,
        460..=469 => Ty::Double,
        470..=479 => Ty::Str,
        480..=481 => Ty::Handle,
        999 => Ty::Comment,
        1005 => Ty::Handle,
        1000..=1009 => Ty::Str,
        1010..=1059 => Ty::Double,
        1060..=1070 => Ty::I16,
        1071 => Ty::I32,
        _ => Ty::Unknown,
    }
}

/// Whether code `c` holds a pointer to another object (not the object's own handle).
pub fn is_pointer(c: i32) -> bool {
    matches!(c, 320..=369 | 390..=399 | 480..=481 | 1005)
}

/// Whether code `c` is in XDATA (`1000` to `1071`).
pub fn is_xdata(c: i32) -> bool {
    (1000..=1071).contains(&c)
}
