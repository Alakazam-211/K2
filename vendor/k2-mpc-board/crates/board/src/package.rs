//! A whole `.k5board`: the manifest, the board and its parts, read and written.

use crate::hash::Sha256;
use crate::json::{self, Fields, Value};
use crate::limits::Limits;
use crate::model::{Board, Lineage};
use crate::parts::PartSource;
use crate::refusal::{Refusal, RefusalCode as C};
use crate::zip::{self, Entry, ZipWriter};
use crate::{FORMAT, MIN_READER, VERSION};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, Write};

pub(crate) const ROLE_BOARD: &str = "board";
pub(crate) const ROLE_BLOB: &str = "blob";
pub(crate) const ROLE_PREVIEW: &str = "preview";
const NAME_MANIFEST: &str = "manifest.json";
const NAME_BOARD: &str = "board.json";
const NAME_PREVIEW: &str = "preview.png";
const MEDIA_MAX_BYTES: usize = 127;

/// One part as the manifest lists it. The writer computes these; a host never sets them.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// Its name in the zip.
    pub name: String,
    /// Its media type.
    pub media_type: String,
    /// `board`, `blob`, `preview`, or a role this version doesn't know, which is kept.
    pub role: String,
    /// Uncompressed size.
    pub bytes: u64,
    /// Hash of the uncompressed bytes.
    pub sha256: Sha256,
    /// Fields this version doesn't know.
    pub extra: Fields,
}

/// The `board.json` a copy left from.
#[derive(Clone, Debug, PartialEq)]
pub struct Base {
    /// Hash of that `board.json`.
    pub sha256: Sha256,
    /// Fields this version doesn't know.
    pub extra: Fields,
}

impl Base {
    /// A base naming this `board.json` hash.
    pub fn new(sha256: Sha256) -> Base {
        Base {
            sha256,
            extra: Fields::new(),
        }
    }
}

/// The manifest as read.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    /// The writer's format version.
    pub version: u32,
    /// The oldest reader version that may read this file.
    pub min_reader: u32,
    /// Tells copies of one board apart.
    pub lineage: Lineage,
    /// The `board.json` this copy left from, if any.
    pub base: Option<Base>,
    /// Every part but the manifest.
    pub parts: Vec<Part>,
    /// Fields this version doesn't know.
    pub extra: Fields,
}

/// All a caller may set in a manifest. The writer computes the parts and the versions, so a
/// manifest can't disagree with what's in the file.
#[derive(Clone, Debug, PartialEq)]
pub struct WriteMeta {
    /// A floor: the file keeps the higher of this and this crate's own version.
    pub version: u32,
    /// A floor: the file keeps the higher of this and this crate's own.
    pub min_reader: u32,
    /// Tells copies of one board apart.
    pub lineage: Lineage,
    /// The `board.json` this copy left from, if any.
    pub base: Option<Base>,
    /// Fields to keep in the manifest.
    pub extra: Fields,
}

impl WriteMeta {
    /// For a new board: the host supplies the lineage's random bytes.
    pub fn new(lineage: Lineage) -> WriteMeta {
        WriteMeta {
            version: VERSION,
            min_reader: MIN_READER,
            lineage,
            base: None,
            extra: Fields::new(),
        }
    }
}

impl Manifest {
    /// What to write back so that an unchanged open and write keeps the guard.
    pub fn write_meta(&self) -> WriteMeta {
        WriteMeta {
            version: self.version,
            min_reader: self.min_reader,
            lineage: self.lineage,
            base: self.base.clone(),
            extra: self.extra.clone(),
        }
    }

    /// The canonical manifest: members sorted, parts sorted by name.
    pub fn to_canonical_json(&self) -> Result<Vec<u8>, Refusal> {
        json::to_canonical(&self.to_value())
    }

    fn to_value(&self) -> Value {
        let mut top = self.extra.clone();
        top.insert("format".into(), Value::Str(FORMAT.into()));
        top.insert("version".into(), Value::Num(f64::from(self.version)));
        top.insert("min_reader".into(), Value::Num(f64::from(self.min_reader)));
        top.insert("lineage".into(), Value::Str(self.lineage.to_hex()));
        if let Some(base) = &self.base {
            let mut b = base.extra.clone();
            b.insert("sha256".into(), Value::Str(base.sha256.to_hex()));
            top.insert("base".into(), Value::Obj(b));
        }
        let mut parts: Vec<&Part> = self.parts.iter().collect();
        parts.sort_by(|a, b| a.name.cmp(&b.name));
        let list = parts
            .iter()
            .map(|p| {
                let mut f = p.extra.clone();
                f.insert("name".into(), Value::Str(p.name.clone()));
                f.insert("media_type".into(), Value::Str(p.media_type.clone()));
                f.insert("role".into(), Value::Str(p.role.clone()));
                f.insert("bytes".into(), Value::Num(p.bytes as f64));
                f.insert("sha256".into(), Value::Str(p.sha256.to_hex()));
                Value::Obj(f)
            })
            .collect();
        top.insert("parts".into(), Value::Arr(list));
        Value::Obj(top)
    }

    fn parse(bytes: &[u8], l: &Limits) -> Result<Manifest, Refusal> {
        let Value::Obj(mut top) = json::parse(bytes, l)? else {
            return Err(bad("/", "the manifest must be an object"));
        };
        match top.remove("format") {
            Some(Value::Str(f)) if f == FORMAT => {}
            _ => {
                return Err(Refusal::at(
                    C::ManifestFormat,
                    "/format",
                    format!("this is not a {FORMAT} file"),
                ))
            }
        }
        // min_reader comes first: a newer format may change anything else below.
        let min_reader = whole(top.remove("min_reader"), "/min_reader")?;
        if min_reader > MIN_READER {
            let mut r = Refusal::at(
                C::MinReaderTooHigh,
                "/min_reader",
                format!("this file needs reader version {min_reader}; this one is {MIN_READER}"),
            );
            r.needs_reader = Some(min_reader);
            return Err(r);
        }
        let version = whole(top.remove("version"), "/version")?;
        if min_reader > version {
            return Err(bad("/min_reader", "min_reader is above version"));
        }
        let lineage = match top.remove("lineage") {
            Some(Value::Str(s)) => Lineage::from_hex(&s)
                .ok_or_else(|| bad("/lineage", "must be 32 lower-case hex digits"))?,
            _ => return Err(bad("/lineage", "must be 32 lower-case hex digits")),
        };
        let base = match top.remove("base") {
            None => None,
            Some(Value::Obj(mut b)) => {
                let sha256 = hash_field(b.remove("sha256"), "/base/sha256")?;
                Some(Base { sha256, extra: b })
            }
            Some(_) => return Err(bad("/base", "must be an object")),
        };
        let Some(Value::Arr(list)) = top.remove("parts") else {
            return Err(bad("/parts", "must be an array"));
        };
        let mut parts = Vec::with_capacity(list.len());
        for (i, v) in list.into_iter().enumerate() {
            parts.push(parse_part(v, &format!("/parts/{i}"))?);
        }
        Ok(Manifest {
            version,
            min_reader,
            lineage,
            base,
            parts,
            extra: top,
        })
    }
}

fn bad(at: &str, what: &str) -> Refusal {
    Refusal::at(C::ManifestInvalid, at, what.to_string())
}

fn whole(v: Option<Value>, at: &str) -> Result<u32, Refusal> {
    match v {
        Some(Value::Num(n)) if n.fract() == 0.0 && (1.0..=f64::from(u32::MAX)).contains(&n) => {
            Ok(n as u32)
        }
        _ => Err(bad(at, "must be a whole number from 1")),
    }
}

fn hash_field(v: Option<Value>, at: &str) -> Result<Sha256, Refusal> {
    match v {
        Some(Value::Str(s)) => {
            Sha256::from_hex(&s).ok_or_else(|| bad(at, "must be 64 lower-case hex digits"))
        }
        _ => Err(bad(at, "must be 64 lower-case hex digits")),
    }
}

fn parse_part(v: Value, at: &str) -> Result<Part, Refusal> {
    let Value::Obj(mut f) = v else {
        return Err(bad(at, "a part must be an object"));
    };
    let mut text = |key: &str| -> Result<String, Refusal> {
        match f.remove(key) {
            Some(Value::Str(s)) => Ok(s),
            _ => Err(bad(&format!("{at}/{key}"), "must be text")),
        }
    };
    let name = text("name")?;
    let media_type = text("media_type")?;
    let role = text("role")?;
    let bytes = match f.remove("bytes") {
        Some(Value::Num(n)) if n.fract() == 0.0 && n >= 0.0 => n as u64,
        _ => return Err(bad(&format!("{at}/bytes"), "must be a whole number")),
    };
    let sha256 = hash_field(f.remove("sha256"), &format!("{at}/sha256"))?;
    Ok(Part {
        name,
        media_type,
        role,
        bytes,
        sha256,
        extra: f,
    })
}

fn media_type_ok(s: &str) -> bool {
    let token = |t: &str| {
        !t.is_empty()
            && t.bytes().all(|c| {
                c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || matches!(c, b'.' | b'+' | b'-' | b'_')
            })
    };
    s.len() <= MEDIA_MAX_BYTES && s.split_once('/').is_some_and(|(a, b)| token(a) && token(b))
}

fn role_ok(s: &str) -> bool {
    crate::validate::is_name(s)
}

/// The rules on a list of parts, the same for a reader and a writer.
fn check_parts(parts: &[Part]) -> Result<(), Refusal> {
    let mut seen = BTreeSet::new();
    let mut boards = 0;
    let mut previews = 0;
    for p in parts {
        let name = p.name.as_str();
        if !zip::name_ok(name) {
            return Err(Refusal::at(
                C::ZipNameInvalid,
                name,
                "not a legal part name",
            ));
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(Refusal::at(C::PartDuplicate, name, "listed twice"));
        }
        if !media_type_ok(&p.media_type) {
            return Err(bad(name, "not a media type"));
        }
        if !role_ok(&p.role) {
            return Err(bad(name, "not a role name"));
        }
        let role_refused = |what: &str| Refusal::at(C::PartRole, name, what.to_string());
        if name == NAME_MANIFEST {
            return Err(role_refused("the manifest doesn't list itself"));
        }
        if (name == NAME_BOARD) != (p.role == ROLE_BOARD) {
            return Err(role_refused(
                "role board goes with board.json, and only there",
            ));
        }
        if (name == NAME_PREVIEW) != (p.role == ROLE_PREVIEW) {
            return Err(role_refused(
                "role preview goes with preview.png, and only there",
            ));
        }
        if name.starts_with("blobs/") != (p.role == ROLE_BLOB) {
            return Err(role_refused("role blob goes under blobs/, and only there"));
        }
        if p.role == ROLE_BOARD {
            boards += 1;
        }
        if p.role == ROLE_PREVIEW {
            previews += 1;
        }
        if p.role == ROLE_BLOB && p.sha256.blob_name() != name {
            return Err(Refusal::at(
                C::PartHash,
                name,
                "a blob's name must be the hash of its bytes",
            ));
        }
    }
    if boards != 1 {
        return Err(Refusal::new(
            C::PartRole,
            format!("{boards} boards; there must be one"),
        ));
    }
    if previews > 1 {
        return Err(Refusal::new(C::PartRole, "more than one preview"));
    }
    Ok(())
}

fn missing_from(board: &Board, parts: &[Part]) -> Vec<Sha256> {
    let names: BTreeSet<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    board
        .blob_refs()
        .into_iter()
        .filter(|h| !names.contains(h.blob_name().as_str()))
        .collect()
}

fn read_manifest<R: Read + Seek>(
    src: &mut R,
    l: &Limits,
) -> Result<(Vec<Entry>, Manifest), Refusal> {
    let entries = zip::read_index(src, l)?;
    let first = entries
        .first()
        .ok_or_else(|| Refusal::new(C::ManifestNotFirst, "the file has no entries"))?;
    // The central directory's first entry, and the one at the file's first byte.
    let lowest = entries.iter().min_by_key(|e| e.data_offset);
    if first.name != NAME_MANIFEST || lowest.map(|e| e.name.as_str()) != Some(NAME_MANIFEST) {
        return Err(Refusal::new(
            C::ManifestNotFirst,
            "manifest.json must be the first entry",
        ));
    }
    let bytes = zip::read_entry(src, first, l)?;
    let manifest = Manifest::parse(&bytes, l)?;
    Ok((entries, manifest))
}

/// The save guard of a file, from its central directory and manifest only: sha256 of the canonical
/// manifest. The manifest lists every part with its hash, so one compare covers any added, removed
/// or changed part. See `docs/FORMAT.md` for what it doesn't cover.
pub fn package_guard_of<R: Read + Seek>(mut src: R, l: &Limits) -> Result<Sha256, Refusal> {
    l.check_ceiling()?;
    let (_, manifest) = read_manifest(&mut src, l)?;
    Ok(Sha256::of(&manifest.to_canonical_json()?))
}

/// An image or other blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    /// Its media type.
    pub media_type: String,
    /// Its bytes, hash checked.
    pub bytes: Vec<u8>,
}

/// An open `.k5board`. The manifest and board are read and checked; other parts are read on demand.
pub struct Package<R> {
    src: R,
    limits: Limits,
    entries: BTreeMap<String, Entry>,
    manifest: Manifest,
    board: Board,
    guard: Sha256,
    board_hash: Sha256,
    missing: Vec<Sha256>,
}

impl<R: Read + Seek> Package<R> {
    /// Opens a file. Everything is judged from the central directory before anything is inflated,
    /// then the manifest and `board.json` are read and checked. Other parts' hashes are checked
    /// when they're read, or all at once with [`Package::verify_parts`].
    pub fn open(mut src: R, l: &Limits) -> Result<Package<R>, Refusal> {
        l.check_ceiling()?;
        let (entries, manifest) = read_manifest(&mut src, l)?;
        check_parts(&manifest.parts)?;
        let by_name: BTreeMap<String, Entry> =
            entries.into_iter().map(|e| (e.name.clone(), e)).collect();
        for p in &manifest.parts {
            let e = by_name.get(&p.name).ok_or_else(|| {
                Refusal::at(C::PartMissing, p.name.as_str(), "listed but not in the zip")
            })?;
            if e.usize_ != p.bytes {
                return Err(Refusal::at(
                    C::PartDisagree,
                    p.name.as_str(),
                    format!("the manifest says {} bytes, the zip {}", p.bytes, e.usize_),
                ));
            }
        }
        for name in by_name.keys() {
            if name != NAME_MANIFEST && !manifest.parts.iter().any(|p| &p.name == name) {
                return Err(Refusal::at(
                    C::PartUnlisted,
                    name.as_str(),
                    "in the zip but not listed",
                ));
            }
        }
        let part = manifest
            .parts
            .iter()
            .find(|p| p.role == ROLE_BOARD)
            .ok_or_else(|| Refusal::new(C::PartRole, "no board"))?;
        let entry = by_name
            .get(NAME_BOARD)
            .ok_or_else(|| Refusal::at(C::PartMissing, NAME_BOARD, "listed but not in the zip"))?;
        let bytes = zip::read_entry(&mut src, entry, l)?;
        let board_hash = Sha256::of(&bytes);
        if board_hash != part.sha256 {
            return Err(Refusal::at(
                C::PartHash,
                NAME_BOARD,
                "the bytes don't match the manifest",
            ));
        }
        let board = Board::from_json(&bytes, l)?;
        let missing = missing_from(&board, &manifest.parts);
        let guard = Sha256::of(&manifest.to_canonical_json()?);
        Ok(Package {
            src,
            limits: *l,
            entries: by_name,
            manifest,
            board,
            guard,
            board_hash,
            missing,
        })
    }

    /// The manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The board.
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// Takes the board out.
    pub fn into_board(self) -> Board {
        self.board
    }

    /// The save guard: sha256 of the canonical manifest. Keep it from the open, and compare it with
    /// [`package_guard_of`] on the file just before replacing it.
    pub fn package_guard(&self) -> Sha256 {
        self.guard
    }

    /// The hash of the file's `board.json`: what `base` names. This is not the guard.
    pub fn board_hash(&self) -> Sha256 {
        self.board_hash
    }

    /// Blobs the board names that aren't in this file. It opens anyway, so a host can draw a
    /// placeholder; `write` refuses until `Op::ClearBlob` clears them.
    pub fn missing_blobs(&self) -> Vec<Sha256> {
        self.missing.clone()
    }

    /// A blob by its hash, hash checked.
    pub fn read_blob(&mut self, h: &Sha256) -> Result<Blob, Refusal> {
        let name = h.blob_name();
        let media_type = self
            .manifest
            .parts
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.media_type.clone())
            .ok_or_else(|| {
                Refusal::at(C::BlobMissing, name.as_str(), "no such part in this file")
            })?;
        Ok(Blob {
            media_type,
            bytes: self.read_part(&name)?,
        })
    }

    /// Checks every part's bytes now.
    pub fn verify_parts(&mut self) -> Result<(), Refusal> {
        let names: Vec<String> = self.manifest.parts.iter().map(|p| p.name.clone()).collect();
        for name in names {
            self.read_part(&name)?;
        }
        Ok(())
    }

    /// One part's bytes by name, hash checked.
    pub fn read_part(&mut self, name: &str) -> Result<Vec<u8>, Refusal> {
        let part = self
            .manifest
            .parts
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| Refusal::at(C::PartMissing, name, "no such part"))?;
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| Refusal::at(C::PartMissing, name, "no such part"))?;
        let bytes = zip::read_entry(&mut self.src, entry, &self.limits)?;
        if Sha256::of(&bytes) != part.sha256 {
            return Err(Refusal::at(
                C::PartHash,
                name,
                "the bytes don't match the manifest",
            ));
        }
        Ok(bytes)
    }
}

impl<R: Read + Seek> PartSource for Package<R> {
    fn list(&self) -> Vec<Part> {
        self.manifest
            .parts
            .iter()
            .filter(|p| p.role != ROLE_BOARD)
            .cloned()
            .collect()
    }

    fn read_part(&mut self, name: &str) -> Result<Vec<u8>, Refusal> {
        Package::read_part(self, name)
    }
}

/// What a write produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    /// The new file's save guard.
    pub package_guard: Sha256,
    /// The hash of the `board.json` it holds: what a later copy's `base` names.
    pub board_hash: Sha256,
    /// The file's size.
    pub bytes: u64,
}

/// Writes a whole file: deterministic, with the board in canonical form and every part the source
/// lists, each checked against its listing. Blobs are never dropped on the writer's own account: a
/// host removes one through [`crate::Overlay`] or [`crate::MemParts`]. On a refusal, part of the
/// output may be written already, so write to a temporary file and replace atomically.
pub fn write<W: Write>(
    out: &mut W,
    meta: &WriteMeta,
    board: &Board,
    parts: &mut dyn PartSource,
    l: &Limits,
) -> Result<Written, Refusal> {
    board.validate(l)?;
    for reserved in [
        "format",
        "version",
        "min_reader",
        "lineage",
        "base",
        "parts",
    ] {
        if meta.extra.contains_key(reserved) {
            return Err(bad(reserved, "the writer sets this itself"));
        }
    }
    let board_json = board.to_canonical_json()?;
    let board_hash = Sha256::of(&board_json);
    let mut listed = parts.list();
    listed.sort_by(|a, b| a.name.cmp(&b.name));

    let board_part = Part {
        name: NAME_BOARD.into(),
        media_type: "application/json".into(),
        role: ROLE_BOARD.into(),
        bytes: board_json.len() as u64,
        sha256: board_hash,
        extra: Fields::new(),
    };
    let mut all = Vec::with_capacity(listed.len() + 1);
    all.push(board_part);
    all.extend(listed.iter().cloned());
    check_parts(&all)?;
    if let Some(h) = missing_from(board, &all).first() {
        return Err(Refusal::at(
            C::BlobMissing,
            h.blob_name(),
            "a dangling reference: clear it with Op::ClearBlob, or supply the blob",
        ));
    }
    check_write_limits(&all, l)?;

    // Never lowered: a newer writer's file keeps saying what it holds.
    let min_reader = meta.min_reader.max(MIN_READER);
    let manifest = Manifest {
        version: meta.version.max(VERSION).max(min_reader),
        min_reader,
        lineage: meta.lineage,
        base: meta.base.clone(),
        parts: all,
        extra: meta.extra.clone(),
    };
    let manifest_json = manifest.to_canonical_json()?;
    if manifest_json.len() as u64 > l.manifest_bytes {
        return Err(Refusal::at(
            C::LimitPartBytes,
            NAME_MANIFEST,
            "the manifest is too large",
        ));
    }

    let mut w = ZipWriter::new(out);
    w.add(
        NAME_MANIFEST,
        zip::METHOD_STORE,
        &manifest_json,
        &manifest_json,
    )?;
    match zip::deflate_for_write(&board_json, l) {
        Some(packed) => w.add(NAME_BOARD, zip::METHOD_DEFLATE, &board_json, &packed)?,
        None => w.add(NAME_BOARD, zip::METHOD_STORE, &board_json, &board_json)?,
    }
    for p in &listed {
        let bytes = parts.read_part(&p.name)?;
        if bytes.len() as u64 != p.bytes {
            return Err(Refusal::at(
                C::PartDisagree,
                p.name.as_str(),
                "not the size it listed",
            ));
        }
        if Sha256::of(&bytes) != p.sha256 {
            return Err(Refusal::at(
                C::PartHash,
                p.name.as_str(),
                "not the hash it listed",
            ));
        }
        w.add(&p.name, zip::METHOD_STORE, &bytes, &bytes)?;
    }
    let size = w.finish()?;
    Ok(Written {
        package_guard: Sha256::of(&manifest_json),
        board_hash,
        bytes: size,
    })
}

fn check_write_limits(all: &[Part], l: &Limits) -> Result<(), Refusal> {
    if all.len() as u64 + 1 > l.entries {
        return Err(Refusal::new(
            C::LimitEntries,
            format!("above {} entries", l.entries),
        ));
    }
    let mut total = 0u64;
    for p in all {
        let max = if p.name == NAME_BOARD {
            l.board_json_bytes
        } else {
            l.part_bytes
        };
        if p.bytes > max {
            return Err(Refusal::at(
                C::LimitPartBytes,
                p.name.as_str(),
                format!("{} bytes, above {max}", p.bytes),
            ));
        }
        total += p.bytes;
    }
    if total > l.total_bytes {
        return Err(Refusal::new(
            C::LimitTotalBytes,
            format!("above {} bytes in all", l.total_bytes),
        ));
    }
    Ok(())
}
