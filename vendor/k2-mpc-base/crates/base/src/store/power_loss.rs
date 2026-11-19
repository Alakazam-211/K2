//! Test builds only: a storage layer for the store that can lose power. Every write lands in
//! what the process reads at once (the OS's page cache, as it were) and in a list of writes not
//! yet synced; a sync makes everything written so far durable. A power loss keeps the durable
//! image and, of the writes since the last sync, a seeded random subset, each one whole or cut
//! short (torn), applied in a random order: what a disk may hold after losing power with writes
//! in flight. The file's length is the durable one or a later one in the order it was set (the
//! file system keeps its own record in order), never one a torn write left. The store is then
//! reopened on that image alone
//! (`DocStore::open_on_backend_for_tests`).

use std::io;
use std::sync::{Arc, Mutex};

/// What the layer holds: what the process reads, the durable image, and the writes since the
/// last sync (each: where, its bytes; a length change is a write of no bytes).
#[derive(Debug, Default)]
struct Image {
    seen: Vec<u8>,
    durable: Vec<u8>,
    unsynced: Vec<Unsynced>,
    /// The next sync fails ([`PowerLoss::fail_next_sync`]).
    fail_sync: bool,
}

#[derive(Debug, Clone)]
enum Unsynced {
    Write { at: u64, bytes: Vec<u8> },
    Len(u64),
}

/// The layer (clones share one image): give a clone to the store, keep one to cut the power.
#[derive(Debug, Clone, Default)]
pub(crate) struct PowerLoss {
    image: Arc<Mutex<Image>>,
}

fn put(v: &mut Vec<u8>, at: u64, bytes: &[u8]) {
    let at = at as usize;
    if v.len() < at + bytes.len() {
        v.resize(at + bytes.len(), 0);
    }
    v[at..at + bytes.len()].copy_from_slice(bytes);
}

/// A small seeded generator (xorshift): the same seed, the same loss.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0.max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

impl PowerLoss {
    /// A layer over `durable` (a power loss's image, or empty for a new store).
    pub(crate) fn from_image(durable: Vec<u8>) -> Self {
        Self {
            image: Arc::new(Mutex::new(Image {
                seen: durable.clone(),
                durable,
                unsynced: Vec::new(),
                fail_sync: false,
            })),
        }
    }

    /// The next sync fails with an I/O error, and the writes since the last one are lost (the
    /// disk dropped them): what the process reads again is the durable image, as a reopen
    /// would find it.
    pub(crate) fn fail_next_sync(&self) {
        self.image.lock().unwrap().fail_sync = true;
    }

    /// The image a power loss now leaves (module docs), from `seed`: the durable image and a
    /// random subset of the writes since the last sync, each whole or torn, in a random order.
    /// `none_unsynced` keeps none of them (the disk lost every write in flight).
    pub(crate) fn lose_power(&self, seed: u64, none_unsynced: bool) -> Vec<u8> {
        let im = self.image.lock().unwrap();
        let mut out = im.durable.clone();
        if none_unsynced {
            return out;
        }
        let mut rng = Rng(seed ^ 0x9e37_79b9_7f4a_7c15);
        let mut order: Vec<usize> = (0..im.unsynced.len()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, (rng.next() % (i as u64 + 1)) as usize);
        }
        for i in order {
            if rng.next() % 2 == 0 {
                continue;
            }
            match &im.unsynced[i] {
                Unsynced::Write { at, bytes } => {
                    // whole, or torn at a random point
                    let keep = if rng.next() % 4 == 0 {
                        (rng.next() % (bytes.len() as u64 + 1)) as usize
                    } else {
                        bytes.len()
                    };
                    put(&mut out, *at, &bytes[..keep]);
                }
                // the file's length is the file system's record, kept in order: below
                Unsynced::Len(_) => {}
            }
        }
        // the length: the durable one, or one of the later set lengths in order (a prefix of
        // them reached the disk), never a length a torn write left; writes past it are cut
        let lens: Vec<u64> = im
            .unsynced
            .iter()
            .filter_map(|u| match u {
                Unsynced::Len(n) => Some(*n),
                Unsynced::Write { .. } => None,
            })
            .collect();
        let k = (rng.next() % (lens.len() as u64 + 1)) as usize;
        let len = match k {
            0 => im.durable.len(),
            k => lens[k - 1] as usize,
        };
        out.resize(len, 0);
        out
    }
}

impl redb::StorageBackend for PowerLoss {
    fn len(&self) -> Result<u64, io::Error> {
        Ok(self.image.lock().unwrap().seen.len() as u64)
    }

    fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, io::Error> {
        let im = self.image.lock().unwrap();
        let (a, b) = (offset as usize, offset as usize + len);
        if b > im.seen.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "a read past the end",
            ));
        }
        Ok(im.seen[a..b].to_vec())
    }

    fn set_len(&self, len: u64) -> Result<(), io::Error> {
        let mut im = self.image.lock().unwrap();
        im.seen.resize(len as usize, 0);
        im.unsynced.push(Unsynced::Len(len));
        Ok(())
    }

    fn sync_data(&self, _eventual: bool) -> Result<(), io::Error> {
        let mut im = self.image.lock().unwrap();
        if std::mem::take(&mut im.fail_sync) {
            im.seen = im.durable.clone();
            im.unsynced.clear();
            return Err(io::Error::other("a planted sync failure"));
        }
        im.durable = im.seen.clone();
        im.unsynced.clear();
        Ok(())
    }

    fn write(&self, offset: u64, data: &[u8]) -> Result<(), io::Error> {
        let mut im = self.image.lock().unwrap();
        put(&mut im.seen, offset, data);
        im.unsynced.push(Unsynced::Write {
            at: offset,
            bytes: data.to_vec(),
        });
        Ok(())
    }
}
