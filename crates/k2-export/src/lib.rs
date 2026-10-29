//! K2's export worker (prd-k2-core-exports-v1 §4.3, vs-live §3): one
//! `k2-export` process per export job, so a decoder that crashes, loops or
//! allocates without bound never takes the daemon with it.
//!
//! - [`proto`]: the wire (stdin header + input, stdout frames);
//! - [`worker`]: the process side (the kit's `ExportRegistry`);
//! - [`host`]: the daemon side (spawn, limits per OS, watchdog, cancel).
//!
//! This crate never links k2-core: no tokens, no DB, no OS integration in
//! the process that parses hostile bytes.

pub mod host;
pub mod proto;
pub mod worker;

#[cfg(feature = "test-hostile")]
pub mod hostile;
