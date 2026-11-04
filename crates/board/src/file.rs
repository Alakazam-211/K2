//! One edit of a file on bytes: open it, apply groups, save behind the guard. A host owns the file,
//! the writer lock and the atomic replace. The flow, under that lock: [`BoardFile::open`] with the
//! [`FileTarget`] the host names, plan on [`BoardFile::board`], [`BoardFile::apply`],
//! [`BoardFile::save`] to a temporary file, [`BoardFile::still_current`], replace. If the file moved
//! on, start again from `open`: each op names the revision it read, so nothing is overwritten unseen.
//!
//! The crate never picks a file: the host names it, and every answer says which file, by which
//! guard, in file mode, as an [`Echoed`]. Live is never implied; it's a separate, later step.
//!
//! The limit: one writer at a time. There's no presence and no live merge here.

use crate::hash::Sha256;
use crate::limits::Limits;
use crate::merge::{merge3, MergeError, Merged};
use crate::model::{Board, Id, Object};
use crate::ops::{apply_group, Conflict, Group, Report};
use crate::package::{package_guard_of, write, Blob, Manifest, Package, Written};
use crate::parts::{Overlay, PartSource};
use crate::refusal::Refusal;
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, Write};

/// The file a host names, exactly as the host gives it. There's no default and no empty one: the
/// host always says which file. The crate never resolves or opens the path; it only echoes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTarget(String);

impl FileTarget {
    /// A target for this path, byte for byte, or `None` for an empty one.
    pub fn new(path: impl Into<String>) -> Option<FileTarget> {
        let path = path.into();
        if path.is_empty() {
            return None;
        }
        Some(FileTarget(path))
    }

    /// The path, as the host gave it.
    pub fn path(&self) -> &str {
        &self.0
    }
}

/// How an answer was reached. Only file mode exists here: live comes later, as its own step, and no
/// file verb can set it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// On a file, with one writer at a time.
    File,
}

impl Mode {
    /// A stable string: `file`.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::File => "file",
        }
    }
}

/// What an answer was about: which kind of file, which file, by which guard, and in which mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct On {
    /// The format's name, [`crate::FORMAT`].
    pub kind: &'static str,
    /// The path the host gave, byte for byte.
    pub path: String,
    /// The package guard, `sha256-…`: the one written after a save that wrote, else the one read.
    /// It's `None` only when `open` refused, since no guard had been read.
    pub version: Option<String>,
    /// Always [`Mode::File`] here.
    pub mode: Mode,
}

impl On {
    fn file(target: &FileTarget, version: Option<Sha256>) -> On {
        On {
            kind: crate::FORMAT,
            path: target.0.clone(),
            version: version.map(|v| v.to_string()),
            mode: Mode::File,
        }
    }
}

/// An answer, with what it was about.
#[derive(Clone, Debug, PartialEq)]
pub struct Echoed<T> {
    /// Which file, by which guard, in which mode.
    pub on: On,
    /// The answer itself.
    pub value: T,
}

/// An open file with the edits applied so far.
pub struct BoardFile<R> {
    target: FileTarget,
    package: Package<R>,
    board: Board,
    staged: BTreeMap<Sha256, (String, Vec<u8>)>,
    limits: Limits,
}

/// A group that didn't apply. Its ops are kept, so the edit isn't lost.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    /// The group, as it came.
    pub group: Group,
    /// Why it didn't apply.
    pub conflict: Conflict,
}

/// What [`BoardFile::apply_all`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    /// The groups that applied, with their reports, in order.
    pub applied: Vec<(String, Report)>,
    /// The groups that didn't. The others went on.
    pub held: Vec<Held>,
}

/// What a save did.
#[derive(Clone, Debug, PartialEq)]
pub enum Saved {
    /// The board didn't change, so nothing was written.
    Nothing,
    /// A whole new file went to the output.
    Wrote {
        /// Its guard, its board hash and its size.
        written: Written,
        /// The new revision of every object added or changed since the file was read.
        revs: BTreeMap<Id, Sha256>,
    },
}

/// Why a save wrote nothing usable.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveError {
    /// The file on disk isn't the one that was read. Nothing was written.
    Stale {
        /// The guard when it was read.
        read: Sha256,
        /// The guard on disk now.
        on_disk: Sha256,
    },
    /// A typed refusal. Part of the output may be written, so write to a temporary file.
    Refused(Refusal),
}

impl From<Refusal> for SaveError {
    fn from(r: Refusal) -> SaveError {
        SaveError::Refused(r)
    }
}

impl<R: Read + Seek> BoardFile<R> {
    /// Opens the file the host names, with every check [`Package::open`] makes. A refusal echoes the
    /// target with no version, since no guard was read.
    pub fn open(
        target: FileTarget,
        src: R,
        limits: &Limits,
    ) -> Result<BoardFile<R>, Box<Echoed<Refusal>>> {
        let package = match Package::open(src, limits) {
            Ok(package) => package,
            Err(value) => {
                return Err(Box::new(Echoed {
                    on: On::file(&target, None),
                    value,
                }))
            }
        };
        let board = package.board().clone();
        Ok(BoardFile {
            target,
            package,
            board,
            staged: BTreeMap::new(),
            limits: *limits,
        })
    }

    /// The board, with the groups applied so far.
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// The manifest as it was read.
    pub fn manifest(&self) -> &Manifest {
        self.package.manifest()
    }

    /// The guard of the file as it was read.
    pub fn guard(&self) -> Sha256 {
        self.package.package_guard()
    }

    /// What this file's answers are about: its kind, the host's path, the guard read, file mode.
    pub fn on(&self) -> On {
        On::file(&self.target, Some(self.guard()))
    }

    /// Any answer about this file, echoed: `file.echo(verbs::read(file.board(), &id))`.
    pub fn echo<T>(&self, value: T) -> Echoed<T> {
        Echoed {
            on: self.on(),
            value,
        }
    }

    fn answer<T, E>(&self, r: Result<T, E>) -> Result<Echoed<T>, Box<Echoed<E>>> {
        match r {
            Ok(v) => Ok(self.echo(v)),
            Err(e) => Err(Box::new(self.echo(e))),
        }
    }

    /// Applies one group, all or nothing, as [`apply_group`].
    pub fn apply(&mut self, group: &Group) -> Result<Echoed<Report>, Box<Echoed<Conflict>>> {
        let r = apply_group(&mut self.board, group);
        self.answer(r)
    }

    /// Applies each group on its own: one that doesn't fit is held whole, and the rest go on.
    pub fn apply_all(&mut self, groups: &[Group]) -> Echoed<Applied> {
        let mut out = Applied::default();
        for group in groups {
            match apply_group(&mut self.board, group) {
                Ok(report) => out.applied.push((group.id.clone(), report)),
                Err(conflict) => out.held.push(Held {
                    group: group.clone(),
                    conflict,
                }),
            }
        }
        self.echo(out)
    }

    /// Merges a copy into this file: `base` is what the copy came from, and `copy_parts` has the
    /// copy's blobs. On a conflict the board is unchanged and every side is kept in the error. A
    /// clean merge is applied here, and saves behind the guard like any other edit.
    pub fn merge(
        &mut self,
        base: &Board,
        ours: &Board,
        copy_parts: &mut dyn PartSource,
    ) -> Result<Echoed<Merged>, Box<Echoed<MergeError>>> {
        let r = self.merge_in(base, ours, copy_parts);
        self.answer(r)
    }

    fn merge_in(
        &mut self,
        base: &Board,
        ours: &Board,
        copy_parts: &mut dyn PartSource,
    ) -> Result<Merged, MergeError> {
        let merged = merge3(base, ours, &self.board)?;
        let there: Vec<String> = self
            .package
            .manifest()
            .parts
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let offered = copy_parts.list();
        for hash in merged.board.blob_refs() {
            let name = hash.blob_name();
            if there.contains(&name) || self.staged.contains_key(&hash) {
                continue;
            }
            if let Some(part) = offered.iter().find(|p| p.name == name) {
                let bytes = copy_parts.read_part(&name)?;
                if Sha256::of(&bytes) != hash {
                    return Err(MergeError::Refused(Refusal::at(
                        crate::refusal::RefusalCode::PartHash,
                        name,
                        "the bytes don't match the name",
                    )));
                }
                self.staged.insert(hash, (part.media_type.clone(), bytes));
            }
        }
        self.board = merged.board.clone();
        Ok(merged)
    }

    /// Stages a picture and returns its hash. It's written only if the board refers to it at save.
    pub fn add_blob(&mut self, media_type: &str, bytes: Vec<u8>) -> Echoed<Sha256> {
        let hash = Sha256::of(&bytes);
        self.staged.insert(hash, (media_type.to_string(), bytes));
        self.echo(hash)
    }

    /// A picture, staged or in the file.
    pub fn read_blob(&mut self, hash: &Sha256) -> Result<Echoed<Blob>, Box<Echoed<Refusal>>> {
        let r = self.blob(hash);
        self.answer(r)
    }

    fn blob(&mut self, hash: &Sha256) -> Result<Blob, Refusal> {
        if let Some((media_type, bytes)) = self.staged.get(hash) {
            return Ok(Blob {
                media_type: media_type.clone(),
                bytes: bytes.clone(),
            });
        }
        self.package.read_blob(hash)
    }

    /// True when the board differs from the file as it was read.
    pub fn is_changed(&self) -> bool {
        self.board != *self.package.board()
    }

    /// Whether the file on disk is still the one that was read. Ask this just before replacing it.
    pub fn still_current<C: Read + Seek>(
        &self,
        on_disk: C,
    ) -> Result<Echoed<bool>, Box<Echoed<Refusal>>> {
        let r = self.current(on_disk);
        self.answer(r)
    }

    fn current<C: Read + Seek>(&self, on_disk: C) -> Result<bool, Refusal> {
        Ok(package_guard_of(on_disk, &self.limits)? == self.guard())
    }

    /// Compares the guard, and writes a whole new file to `out` if the board changed. On
    /// [`SaveError::Stale`] and [`Saved::Nothing`], `out` is left untouched. The echo's version is
    /// the guard written, or the one read when nothing was written.
    pub fn save<C: Read + Seek, W: Write>(
        self,
        on_disk: C,
        out: &mut W,
    ) -> Result<Echoed<Saved>, Box<Echoed<SaveError>>> {
        let (target, read) = (self.target.clone(), self.guard());
        let r = self.write_to(on_disk, out);
        let version = match &r {
            Ok(Saved::Wrote { written, .. }) => written.package_guard,
            _ => read,
        };
        let on = On::file(&target, Some(version));
        match r {
            Ok(value) => Ok(Echoed { on, value }),
            Err(value) => Err(Box::new(Echoed { on, value })),
        }
    }

    fn write_to<C: Read + Seek, W: Write>(
        mut self,
        on_disk: C,
        out: &mut W,
    ) -> Result<Saved, SaveError> {
        let read = self.guard();
        let now = package_guard_of(on_disk, &self.limits)?;
        if now != read {
            return Err(SaveError::Stale { read, on_disk: now });
        }
        if !self.is_changed() {
            return Ok(Saved::Nothing);
        }
        let revs = self.new_revs()?;
        let meta = self.package.manifest().write_meta();
        let refs = self.board.blob_refs();
        let there: Vec<String> = self
            .package
            .manifest()
            .parts
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let staged = std::mem::take(&mut self.staged);
        let mut parts = Overlay::new(&mut self.package);
        for (hash, (media_type, bytes)) in staged {
            if refs.contains(&hash) && !there.contains(&hash.blob_name()) {
                parts.add_blob(&media_type, bytes);
            }
        }
        let written = write(out, &meta, &self.board, &mut parts, &self.limits)?;
        Ok(Saved::Wrote { written, revs })
    }

    fn new_revs(&self) -> Result<BTreeMap<Id, Sha256>, Refusal> {
        let before: HashMap<&Id, &Object> = self
            .package
            .board()
            .objects
            .iter()
            .map(|o| (&o.id, o))
            .collect();
        let mut revs = BTreeMap::new();
        for o in &self.board.objects {
            if before.get(&o.id).copied() != Some(o) {
                revs.insert(o.id.clone(), o.rev()?);
            }
        }
        Ok(revs)
    }
}
