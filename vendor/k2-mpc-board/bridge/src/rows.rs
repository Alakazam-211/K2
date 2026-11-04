//! Which of the kit's rows each refusal of the board crate is.

use crate::bug::HostBug;
use k2_mpc_base::errors::{lookup, ErrorEntry};
use k2_mpc_board::verbs::VerbCode;
use k2_mpc_board::{Conflict, Op, RefusalCode, Target};

/// The call a refusal came from. A format refusal's row depends on it: read from the file it's the
/// file's (exit 4), in the caller's input or a write it's the caller's (exit 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Opening the file.
    Open,
    /// Reading the guard of the file on disk, before a save.
    GuardRead,
    /// Reading raw ops (`apply`'s).
    Ops,
    /// The caller's other input: a verb's values, a group applied, a merge's result.
    Input,
    /// Writing the file.
    Write,
}

impl Phase {
    /// Every phase.
    pub const ALL: &'static [Phase] = &[
        Phase::Open,
        Phase::GuardRead,
        Phase::Ops,
        Phase::Input,
        Phase::Write,
    ];
}

/// What a conflict says happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Clash {
    /// It changed since the caller read it.
    Changed,
    /// The id or token is in use.
    Exists,
}

/// What a conflict is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TargetKind {
    /// An object.
    Object,
    /// The board's settings.
    Settings,
    /// A person.
    Person,
}

/// The kit's row by id.
pub fn row(id: &'static str) -> Result<&'static ErrorEntry, HostBug> {
    lookup(id).ok_or(HostBug::NoRow { id })
}

/// A format refusal's row, by the call it came from (rulings of 11-04):
///
/// - the package, its manifest and parts, a newer format, a missing picture, a lock and I/O have
///   one row each, whatever the call;
/// - a limit is the file's (MPC-E846) when read from the file, else the caller's (MPC-E848);
///   `limit_raised` is always the caller's;
/// - JSON and the board's shape are the file's (MPC-E845) when read from the file, raw ops'
///   (MPC-E849) in `apply`'s ops, else a value the caller gave (MPC-E847);
/// - ids, types and fields are the file's (MPC-E845) when read from the file, else a value the
///   caller gave (MPC-E847).
///
/// `RefusalCode` is `non_exhaustive`, so a code this bridge doesn't know is a [`HostBug`]; the
/// tests hold every code the board crate lists to its ruled row.
pub fn refusal_row(code: RefusalCode, phase: Phase) -> Result<&'static ErrorEntry, HostBug> {
    use RefusalCode as C;
    let file = matches!(phase, Phase::Open | Phase::GuardRead);
    let id = match code {
        C::ZipNoEndRecord
        | C::ZipStrayBytes
        | C::ZipOverlap
        | C::ZipMultiDisk
        | C::Zip64
        | C::ZipEncrypted
        | C::ZipFlags
        | C::ZipMethod
        | C::ZipMalformed
        | C::ZipHeaderMismatch
        | C::ZipNameInvalid
        | C::ZipNameDuplicate
        | C::ZipNotRegular
        | C::ZipCrc
        | C::ZipSizeMismatch
        | C::ZipDeflateInvalid => "MPC-E842",
        C::ManifestNotFirst
        | C::ManifestInvalid
        | C::ManifestFormat
        | C::PartMissing
        | C::PartUnlisted
        | C::PartDuplicate
        | C::PartDisagree
        | C::PartHash
        | C::PartRole => "MPC-E843",
        C::MinReaderTooHigh => "MPC-E844",
        C::BlobMissing => "MPC-E841",
        C::LimitRaised => "MPC-E848",
        C::LimitEntries
        | C::LimitPartBytes
        | C::LimitTotalBytes
        | C::LimitRatio
        | C::LimitObjects
        | C::LimitPeople
        | C::LimitDepth
        | C::LimitValues
        | C::LimitNumberLiteral
        | C::LimitField => {
            if file {
                "MPC-E846"
            } else {
                "MPC-E848"
            }
        }
        C::JsonUtf8
        | C::JsonSyntax
        | C::JsonDuplicateKey
        | C::JsonSurrogate
        | C::JsonNumberInexact
        | C::JsonNumberRange
        | C::BoardShape => {
            if file {
                "MPC-E845"
            } else if phase == Phase::Ops {
                "MPC-E849"
            } else {
                "MPC-E847"
            }
        }
        C::IdInvalid
        | C::IdDuplicate
        | C::TypeInvalid
        | C::FieldNameInvalid
        | C::FieldKind
        | C::FieldValue
        | C::PersonUnknown
        | C::FramesMismatch
        | C::CellColumnUnknown => {
            if file {
                "MPC-E845"
            } else {
                "MPC-E847"
            }
        }
        C::ObjectLocked => "MPC-E835",
        C::IoError => "MPC-E857",
        _ => {
            return Err(HostBug::Unmapped {
                code: code.as_str(),
            })
        }
    };
    row(id)
}

/// A verb's own refusal's row: one each.
pub fn verb_code_row(code: VerbCode) -> Result<&'static ErrorEntry, HostBug> {
    row(match code {
        VerbCode::NeedsActor => "MPC-E855",
        VerbCode::VotingOff => "MPC-E836",
        VerbCode::NoVotesLeft => "MPC-E837",
        VerbCode::WrongType => "MPC-E853",
        VerbCode::TooFew => "MPC-E854",
        VerbCode::NoTemplate => "MPC-E839",
        VerbCode::UnknownField => "MPC-E850",
        VerbCode::UnknownType => "MPC-E851",
        VerbCode::NoSuchItem => "MPC-E840",
        VerbCode::NoFreeId => "MPC-E856",
        VerbCode::BadValue => "MPC-E852",
    })
}

/// What a target is.
pub fn target_kind(t: &Target) -> TargetKind {
    match t {
        Target::Object(_) => TargetKind::Object,
        Target::Settings => TargetKind::Settings,
        Target::Person(_) => TargetKind::Person,
    }
}

/// The two conflicts the board crate never gives, a change to a person and an add of the settings,
/// as the [`HostBug`] each is, whatever the call: they have no row. Every other conflict is `Ok`.
pub fn impossible_pair(c: &Conflict) -> Result<(), HostBug> {
    let (clash, target) = match c {
        Conflict::Changed {
            target: Target::Person(_),
            ..
        } => ("changed", "person"),
        Conflict::Exists {
            target: Target::Settings,
            ..
        } => ("exists", "settings"),
        _ => return Ok(()),
    };
    Err(HostBug::ImpossibleConflict { clash, target })
}

/// A conflict's row, by what it's about: a changed object (MPC-E830) or the settings
/// (MPC-E858), an object id in use (MPC-E831) or a person's token (MPC-E859), an object gone
/// (MPC-E838). A refused group takes its refusal's row, by `phase`. The two pairs the board crate
/// never gives are a [`HostBug`] ([`impossible_pair`]).
pub fn conflict_row(c: &Conflict, phase: Phase) -> Result<&'static ErrorEntry, HostBug> {
    impossible_pair(c)?;
    row(match c {
        Conflict::Changed {
            target: Target::Settings,
            ..
        } => "MPC-E858",
        Conflict::Changed { .. } => "MPC-E830",
        Conflict::Exists {
            target: Target::Person(_),
            ..
        } => "MPC-E859",
        Conflict::Exists { .. } => "MPC-E831",
        Conflict::Deleted { .. } => "MPC-E838",
        Conflict::Refused(r) => return refusal_row(r.code, phase),
    })
}

/// The conflicts each op can give, other than a refusal or a deleted object, by what they say and
/// what they're about. Matched with no wildcard arm: a new op doesn't compile until it's listed,
/// and the tests fail if the board crate ever gives a pair this table doesn't name.
pub fn op_clashes(op: &Op) -> &'static [(Clash, TargetKind)] {
    match op {
        Op::Add { .. } => &[(Clash::Exists, TargetKind::Object)],
        Op::Set { .. } | Op::Del { .. } => &[(Clash::Changed, TargetKind::Object)],
        Op::Meta { .. } => &[(Clash::Changed, TargetKind::Settings)],
        Op::AddPerson { .. } => &[(Clash::Exists, TargetKind::Person)],
        Op::Z { .. } | Op::FrameOrder { .. } | Op::ClearBlob { .. } => &[],
    }
}
