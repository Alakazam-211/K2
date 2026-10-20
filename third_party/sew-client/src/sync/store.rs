//! Cursor stores (S25 §13.3). `JsonFileStore` writes a temp file, fsyncs, renames (mode 0600).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::error::ClientError;

use super::state::{SYNC_STATE_FORMAT, SyncState};

/// Loads and saves a [`SyncState`]. K2 implements it over its own database.
pub trait CursorStore: Send {
    /// The stored state; a store that holds nothing returns `SyncState::default()`.
    fn load(&mut self) -> Result<SyncState, ClientError>;
    /// Replaces the stored state (atomically).
    fn save(&mut self, s: &SyncState) -> Result<(), ClientError>;
}

/// A JSON file: `save` writes `<path>.tmp`, fsyncs it, renames it over `<path>` and fsyncs the
/// directory; the file mode is `0600`. `load` of a missing file is the default state; a file
/// that does not parse or has `format ≠ 1` is a `Config` error (fail loudly; `sewc sync --reset`
/// deletes it).
#[derive(Debug, Clone)]
pub struct JsonFileStore {
    path: PathBuf,
}

impl JsonFileStore {
    /// A store at `path` (the parent directory must exist).
    pub fn new(path: impl Into<PathBuf>) -> JsonFileStore {
        JsonFileStore { path: path.into() }
    }

    /// The file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn tmp_path(&self) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(".tmp");
        PathBuf::from(p)
    }
}

fn io_err(path: &Path, e: std::io::Error) -> ClientError {
    ClientError::Config(format!("{}: {e}", path.display()))
}

impl CursorStore for JsonFileStore {
    fn load(&mut self) -> Result<SyncState, ClientError> {
        let bytes = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SyncState::default()),
            Err(e) => return Err(io_err(&self.path, e)),
        };
        let s: SyncState = serde_json::from_slice(&bytes).map_err(|e| {
            ClientError::Config(format!(
                "{}: not a sync state file ({e}); delete it to start over",
                self.path.display()
            ))
        })?;
        if s.format != SYNC_STATE_FORMAT {
            return Err(ClientError::Config(format!(
                "{}: sync state format {} is not {SYNC_STATE_FORMAT}; delete it to start over",
                self.path.display(),
                s.format
            )));
        }
        Ok(s)
    }

    fn save(&mut self, s: &SyncState) -> Result<(), ClientError> {
        let bytes = serde_json::to_vec_pretty(s)
            .map_err(|e| ClientError::Config(format!("sync state does not serialize: {e}")))?;
        let tmp = self.tmp_path();
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|e| io_err(&tmp, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
                .map_err(|e| io_err(&tmp, e))?;
        }
        f.write_all(&bytes).map_err(|e| io_err(&tmp, e))?;
        f.sync_all().map_err(|e| io_err(&tmp, e))?;
        drop(f);
        fs::rename(&tmp, &self.path).map_err(|e| io_err(&self.path, e))?;
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty())
            && let Ok(d) = fs::File::open(dir)
        {
            // Best effort: some filesystems refuse fsync on a directory.
            let _ = d.sync_all();
        }
        Ok(())
    }
}

/// An in-memory store (tests, short-lived callers).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryStore(pub SyncState);

impl CursorStore for MemoryStore {
    fn load(&mut self) -> Result<SyncState, ClientError> {
        Ok(self.0.clone())
    }

    fn save(&mut self, s: &SyncState) -> Result<(), ClientError> {
        self.0 = s.clone();
        Ok(())
    }
}
