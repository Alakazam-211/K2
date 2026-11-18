//! The command families the binary answers itself, one module each. Each
//! does what the shell CLI's command does, byte for byte: the same
//! requests, the same output and the same exit code.
//!
//! - [`whoami`]: `k5 whoami [--json]`.

pub mod whoami;
