//! Where a writer gets the parts other than `board.json`: blobs, a preview, and parts with a role
//! this version doesn't know. A part changes only when a host says so.

use crate::hash::Sha256;
use crate::json::Fields;
use crate::package::{Part, ROLE_BLOB, ROLE_PREVIEW};
use crate::refusal::{Refusal, RefusalCode};
use std::collections::{BTreeMap, BTreeSet};

/// Every part but `board.json`. A writer checks each part's bytes against its listing.
pub trait PartSource {
    /// The parts, with their names, roles, media types, sizes and hashes.
    fn list(&self) -> Vec<Part>;
    /// One part's bytes.
    fn read_part(&mut self, name: &str) -> Result<Vec<u8>, Refusal>;
}

/// Parts held in memory.
#[derive(Clone, Debug, Default)]
pub struct MemParts {
    parts: BTreeMap<String, (Part, Vec<u8>)>,
}

impl MemParts {
    /// None yet.
    pub fn new() -> MemParts {
        MemParts::default()
    }

    /// Adds a blob, named by its hash, and returns the hash.
    pub fn add_blob(&mut self, media_type: &str, bytes: Vec<u8>) -> Sha256 {
        let h = Sha256::of(&bytes);
        self.add(&h.blob_name(), ROLE_BLOB, media_type, bytes, Fields::new());
        h
    }

    /// Sets the preview. A preview is a picture of the board at some moment; the host decides when
    /// to replace it.
    pub fn set_preview(&mut self, bytes: Vec<u8>) {
        self.add(
            "preview.png",
            ROLE_PREVIEW,
            "image/png",
            bytes,
            Fields::new(),
        );
    }

    /// Adds any part. A writer checks the name and the role.
    pub fn add(&mut self, name: &str, role: &str, media_type: &str, bytes: Vec<u8>, extra: Fields) {
        let part = Part {
            name: name.to_string(),
            media_type: media_type.to_string(),
            role: role.to_string(),
            bytes: bytes.len() as u64,
            sha256: Sha256::of(&bytes),
            extra,
        };
        self.parts.insert(name.to_string(), (part, bytes));
    }

    /// Removes a part. True if it was there.
    pub fn remove(&mut self, name: &str) -> bool {
        self.parts.remove(name).is_some()
    }
}

impl PartSource for MemParts {
    fn list(&self) -> Vec<Part> {
        self.parts.values().map(|(p, _)| p.clone()).collect()
    }

    fn read_part(&mut self, name: &str) -> Result<Vec<u8>, Refusal> {
        self.parts
            .get(name)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| Refusal::at(RefusalCode::PartMissing, name, "no such part"))
    }
}

/// Another source with changes on top. Everything passes through as it came, unknown roles and a
/// preview included, except what's added or removed here.
pub struct Overlay<'a> {
    base: &'a mut dyn PartSource,
    added: MemParts,
    removed: BTreeSet<String>,
}

impl<'a> Overlay<'a> {
    /// Nothing changed yet.
    pub fn new(base: &'a mut dyn PartSource) -> Overlay<'a> {
        Overlay {
            base,
            added: MemParts::new(),
            removed: BTreeSet::new(),
        }
    }

    /// Adds a blob and returns its hash.
    pub fn add_blob(&mut self, media_type: &str, bytes: Vec<u8>) -> Sha256 {
        let h = self.added.add_blob(media_type, bytes);
        self.removed.remove(&h.blob_name());
        h
    }

    /// Replaces the preview.
    pub fn set_preview(&mut self, bytes: Vec<u8>) {
        self.added.set_preview(bytes);
        self.removed.remove("preview.png");
    }

    /// Takes a part out of what gets written.
    pub fn remove(&mut self, name: &str) {
        self.added.remove(name);
        self.removed.insert(name.to_string());
    }
}

impl PartSource for Overlay<'_> {
    fn list(&self) -> Vec<Part> {
        let mine = self.added.list();
        let mut out: Vec<Part> = self
            .base
            .list()
            .into_iter()
            .filter(|p| !self.removed.contains(&p.name) && !mine.iter().any(|m| m.name == p.name))
            .collect();
        out.extend(mine);
        out
    }

    fn read_part(&mut self, name: &str) -> Result<Vec<u8>, Refusal> {
        if self.removed.contains(name) {
            return Err(Refusal::at(RefusalCode::PartMissing, name, "removed"));
        }
        if self.added.list().iter().any(|p| p.name == name) {
            return self.added.read_part(name);
        }
        self.base.read_part(name)
    }
}
