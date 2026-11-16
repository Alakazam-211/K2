//! What every translator shares while it reads: the document's keys, its media (one entry per
//! distinct reference), and the count of model nodes against the reader's limit.

use std::collections::HashMap;

use crate::error::{ReadError, ReadLimits};
use crate::model::{key_token, Key, KeyBook, Media, MediaKind};
use crate::time::Time;

pub(crate) struct Builder {
    pub(crate) book: KeyBook,
    pub(crate) media: Vec<Media>,
    by_identity: HashMap<String, usize>,
    nodes: u64,
    pub(crate) limits: ReadLimits,
}

impl Builder {
    pub(crate) fn new(limits: &ReadLimits) -> Self {
        Builder {
            book: KeyBook::default(),
            media: Vec::new(),
            by_identity: HashMap::new(),
            nodes: 0,
            limits: *limits,
        }
    }

    /// One more model node; refused past the limit.
    pub(crate) fn count(&mut self) -> Result<(), ReadError> {
        self.nodes += 1;
        if self.nodes > self.limits.max_items {
            return Err(ReadError::Limit {
                what: "timeline nodes",
                limit: self.limits.max_items,
            });
        }
        Ok(())
    }

    /// Refused when `depth` is past the limit.
    pub(crate) fn depth(&self, depth: u32) -> Result<(), ReadError> {
        if depth > self.limits.max_depth {
            return Err(ReadError::Limit {
                what: "nesting levels",
                limit: u64::from(self.limits.max_depth),
            });
        }
        Ok(())
    }

    /// The media entry for `identity` (what makes two references the same media: the
    /// format's ID, else kind and location): the existing one's key, or a new entry
    /// (`native` the format's own ID for it, if any) made by `fill`.
    pub(crate) fn media(
        &mut self,
        identity: String,
        native: Option<&str>,
        name: &str,
        kind: MediaKind,
        fill: impl FnOnce(&mut Media),
    ) -> Result<Key, ReadError> {
        if let Some(&i) = self.by_identity.get(&identity) {
            return Ok(self.media[i].key.clone());
        }
        self.count()?;
        let key = match native {
            Some(id) => self.book.key(None, "media", id, true),
            None => self.book.key(None, "d-media", &key_token(name), false),
        };
        let mut m = Media::new(key.clone(), kind, name);
        fill(&mut m);
        self.by_identity.insert(identity, self.media.len());
        self.media.push(m);
        Ok(key)
    }
}

/// `start + (at - origin)`: a time in an owner's media time, placed in record time.
pub(crate) fn place(start: Time, at: Time, origin: Time) -> Result<Time, ReadError> {
    Ok(start.checked_add(at.checked_sub(origin)?)?)
}
