//! Typed refusals. Every way this crate declines a file or a value is a code here, with a stable
//! string that the vectors, the schema and other languages use.

use std::fmt;

macro_rules! codes {
    ($($variant:ident => $text:literal,)*) => {
        /// Why something was refused. The strings from [`RefusalCode::as_str`] are stable.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum RefusalCode { $(#[doc = $text] $variant,)* }

        impl RefusalCode {
            /// Every code.
            pub const ALL: &'static [RefusalCode] = &[$(RefusalCode::$variant,)*];

            /// The stable string for this code.
            pub fn as_str(self) -> &'static str {
                match self { $(RefusalCode::$variant => $text,)* }
            }
        }
    };
}

codes! {
    ZipNoEndRecord => "zip_no_end_record",
    ZipStrayBytes => "zip_stray_bytes",
    ZipOverlap => "zip_overlap",
    ZipMultiDisk => "zip_multi_disk",
    Zip64 => "zip64",
    ZipEncrypted => "zip_encrypted",
    ZipFlags => "zip_flags",
    ZipMethod => "zip_method",
    ZipMalformed => "zip_malformed",
    ZipHeaderMismatch => "zip_header_mismatch",
    ZipNameInvalid => "zip_name_invalid",
    ZipNameDuplicate => "zip_name_duplicate",
    ZipNotRegular => "zip_not_regular",
    ZipCrc => "zip_crc",
    ZipSizeMismatch => "zip_size_mismatch",
    ZipDeflateInvalid => "zip_deflate_invalid",
    ManifestNotFirst => "manifest_not_first",
    ManifestInvalid => "manifest_invalid",
    ManifestFormat => "manifest_format",
    MinReaderTooHigh => "min_reader_too_high",
    PartMissing => "part_missing",
    PartUnlisted => "part_unlisted",
    PartDuplicate => "part_duplicate",
    PartDisagree => "part_disagree",
    PartHash => "part_hash",
    PartRole => "part_role",
    BlobMissing => "blob_missing",
    LimitEntries => "limit_entries",
    LimitPartBytes => "limit_part_bytes",
    LimitTotalBytes => "limit_total_bytes",
    LimitRatio => "limit_ratio",
    LimitObjects => "limit_objects",
    LimitPeople => "limit_people",
    LimitDepth => "limit_depth",
    LimitValues => "limit_values",
    LimitNumberLiteral => "limit_number_literal",
    LimitField => "limit_field",
    LimitRaised => "limit_raised",
    JsonUtf8 => "json_utf8",
    JsonSyntax => "json_syntax",
    JsonDuplicateKey => "json_duplicate_key",
    JsonSurrogate => "json_surrogate",
    JsonNumberInexact => "json_number_inexact",
    JsonNumberRange => "json_number_range",
    BoardShape => "board_shape",
    IdInvalid => "id_invalid",
    IdDuplicate => "id_duplicate",
    TypeInvalid => "type_invalid",
    FieldNameInvalid => "field_name_invalid",
    FieldKind => "field_kind",
    FieldValue => "field_value",
    PersonUnknown => "person_unknown",
    FramesMismatch => "frames_mismatch",
    CellColumnUnknown => "cell_column_unknown",
    ObjectLocked => "object_locked",
    IoError => "io_error",
}

impl fmt::Display for RefusalCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A refusal: nothing was cut, rounded or dropped to get past it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// Why.
    pub code: RefusalCode,
    /// Where: a part name, a JSON pointer, or a byte offset, when there is one.
    pub at: Option<String>,
    /// A sentence for people. Not stable; match on `code`.
    pub detail: String,
    /// For [`RefusalCode::MinReaderTooHigh`]: the reader version the file asks for.
    pub needs_reader: Option<u32>,
    /// For [`RefusalCode::IdInvalid`]: every id that breaks the rule, once each, up to
    /// [`crate::schema::BAD_IDS_MAX`].
    pub bad_ids: Vec<String>,
    /// For [`RefusalCode::IdInvalid`]: how many ids break the rule, past the cap too.
    pub bad_ids_total: u64,
    /// For [`RefusalCode::IdInvalid`]: whether `bad_ids` stops short of `bad_ids_total`.
    pub bad_ids_truncated: bool,
}

impl Refusal {
    /// A refusal with no location.
    pub fn new(code: RefusalCode, detail: impl Into<String>) -> Refusal {
        Refusal {
            code,
            at: None,
            detail: detail.into(),
            needs_reader: None,
            bad_ids: Vec::new(),
            bad_ids_total: 0,
            bad_ids_truncated: false,
        }
    }

    /// A refusal at a location.
    pub fn at(code: RefusalCode, at: impl Into<String>, detail: impl Into<String>) -> Refusal {
        Refusal {
            code,
            at: Some(at.into()),
            detail: detail.into(),
            needs_reader: None,
            bad_ids: Vec::new(),
            bad_ids_total: 0,
            bad_ids_truncated: false,
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.at {
            Some(at) => write!(f, "{} at {}: {}", self.code, at, self.detail),
            None => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl std::error::Error for Refusal {}
