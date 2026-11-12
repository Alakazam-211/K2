//! The bounds on untrusted input. Every one is checked before the memory it guards is
//! allocated.

/// Bounds on a package and its XML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest file, in bytes.
    pub max_file: usize,
    /// The most entries in a package.
    pub max_entries: usize,
    /// The longest entry name, in bytes.
    pub max_name_len: usize,
    /// The largest uncompressed entry, in bytes.
    pub max_entry: u64,
    /// The largest uncompressed total, in bytes.
    pub max_total: u64,
    /// The largest ratio of uncompressed to compressed size of an entry...
    pub max_ratio: u64,
    /// ...once the entry is at least this big, in bytes.
    pub ratio_floor: u64,
    /// The most XML nodes in a parsed part.
    pub max_nodes: u32,
    /// The deepest XML nesting in a parsed part.
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_file: 64 << 20,
            max_entries: 4096,
            max_name_len: 512,
            max_entry: 64 << 20,
            max_total: 256 << 20,
            max_ratio: 100,
            ratio_floor: 1 << 20,
            max_nodes: 4_000_000,
            max_depth: 256,
        }
    }
}
